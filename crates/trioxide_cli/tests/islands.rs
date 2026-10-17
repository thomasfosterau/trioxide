//! `trioxide build` with WASM islands (supplied through the hidden `--debug-island` hook):
//! emitted files, `--emit=wasm|rust`, and the fallback when `cargo` is missing from PATH.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn island_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/islands")
        .join(name)
        .canonicalize()
        .unwrap()
}

/// A config file pointing the island cache at `cache`.
fn config(dir: &Path, cache: &Path) -> PathBuf {
    let path = dir.join("trioxide.json");
    std::fs::write(&path, serde_json::json!({ "cacheDir": cache }).to_string()).unwrap();
    path
}

fn shared_cache() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("island-cache")
}

/// Runs `trioxide build` on the `nbody` island fixture, writing into `out`.
fn build(out: &Path, cache: &Path, extra: &[&str], path: Option<OsString>) -> Output {
    let fixture = island_fixture("nbody");
    let config = config(out.parent().unwrap(), cache);
    let mut command = Command::new(env!("CARGO_BIN_EXE_trioxide"));
    command
        .arg("build")
        .args(extra)
        .arg("--config")
        .arg(&config)
        .arg("--root")
        .arg(&fixture)
        .arg("--debug-island")
        .arg(fixture.join("island"))
        .arg("--out-dir")
        .arg(out)
        .arg(fixture.join("input.ts"));
    if let Some(path) = path {
        command.env("PATH", path);
    }
    command.output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A directory with `rustc` (from the real PATH) but no `cargo`.
#[cfg(unix)]
fn path_without_cargo(dir: &Path) -> OsString {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let rustc = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("rustc"))
        .find(|path| path.is_file())
        .unwrap();
    std::os::unix::fs::symlink(rustc, bin.join("rustc")).unwrap();
    bin.into_os_string()
}

#[test]
fn emits_glue_and_wasm_next_to_the_js() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let output = build(&out, &shared_cache(), &["--require-wasm"], None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    assert!(out.join("input.js").is_file() && out.join("kernel.js").is_file());
    let glue = std::fs::read_to_string(out.join("_islands/nbody.js")).unwrap();
    assert!(glue.contains("new URL(\"./nbody.wasm\", import.meta.url)"));
    let wasm = std::fs::read(out.join("_islands/nbody.wasm")).unwrap();
    assert_eq!(&wasm[..4], b"\0asm");
}

#[test]
fn emit_wasm_writes_only_islands() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let output = build(&out, &shared_cache(), &["--emit=wasm"], None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(out.join("_islands/nbody.wasm").is_file() && out.join("_islands/nbody.js").is_file());
    assert!(!out.join("input.js").exists());
}

/// `--emit=rust` writes a crate that builds on its own, without needing cargo itself.
#[cfg(unix)]
#[test]
fn emit_rust_writes_the_island_crate_without_cargo() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let path = path_without_cargo(dir.path());
    let output = build(&out, &shared_cache(), &["--emit=rust"], Some(path));
    assert!(output.status.success(), "{}", stderr(&output));
    let crate_dir = out.join("_islands/nbody");
    let lib = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap();
    assert!(lib.contains("export_name = \"advance\""));
    let spec = std::fs::read_to_string(crate_dir.join("island.json")).unwrap();
    assert!(spec.contains("\"id\": \"nbody\""));
    let manifest = std::fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap();
    let runtime = manifest
        .lines()
        .find_map(|line| line.strip_prefix("trioxide_rt = { path = \""))
        .and_then(|rest| rest.split('"').next())
        .unwrap();
    assert!(
        Path::new(runtime).join("src/abi.rs").is_file(),
        "{manifest}"
    );
    assert!(!out.join("input.js").exists());
}

/// With `cargo` hidden from PATH and nothing cached, the build still succeeds: one warning,
/// the JS is written, and no island files are. `--require-wasm` makes it fail instead.
#[cfg(unix)]
#[test]
fn missing_cargo_falls_back_to_js() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let path = path_without_cargo(dir.path());

    let out = dir.path().join("fallback");
    let output = build(&out, &cache, &[], Some(path.clone()));
    let stderr_text = stderr(&output);
    assert!(output.status.success(), "{stderr_text}");
    assert_eq!(stderr_text.matches("warning").count(), 1, "{stderr_text}");
    assert!(
        stderr_text.contains("`cargo` was not found on PATH"),
        "{stderr_text}"
    );
    assert!(
        stderr_text.contains("1 WASM island(s) fall back to JS: nbody"),
        "{stderr_text}"
    );
    assert!(out.join("input.js").is_file() && out.join("kernel.js").is_file());
    assert!(!out.join("_islands").exists());

    let out = dir.path().join("required");
    let output = build(&out, &cache, &["--require-wasm"], Some(path));
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("--require-wasm"),
        "{}",
        stderr(&output)
    );
    assert!(!out.join("input.js").exists());
}
