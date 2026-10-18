//! End-to-end tests of the `trioxide` binary.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn trioxide(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trioxide"))
        .args(args)
        .output()
        .unwrap()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn stdout(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).unwrap()
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).unwrap()
}

/// With no optimisation passes enabled yet, `transform` must print exactly what plain type
/// stripping prints. Once passes are on by default (S4), the differential suite is the check.
#[test]
fn transform_matches_plain_stripping() {
    for name in [
        "classes",
        "enums",
        "imports",
        "jsx",
        "decorators",
        "class-fields-legacy",
    ] {
        let dir = fixture(name);
        let input = ["input.ts", "input.tsx"]
            .iter()
            .map(|file| dir.join(file))
            .find(|path| path.is_file())
            .unwrap();
        let input = input.to_str().unwrap();
        let transformed = trioxide(&["transform", input]);
        let stripped = trioxide(&["strip", input]);
        assert!(
            transformed.status.success(),
            "{name}: {}",
            stderr(&transformed)
        );
        assert!(stripped.status.success(), "{name}: {}", stderr(&stripped));
        assert_eq!(stdout(&transformed), stdout(&stripped), "{name}");
        assert!(
            !stdout(&transformed).contains(": number"),
            "{name}: types were not stripped"
        );
    }
}

#[test]
fn transform_honours_the_fixture_tsconfig() {
    let out = trioxide(&["transform", fixture("jsx/input.tsx").to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("h(List, {"), "{}", stdout(&out));
}

#[test]
fn syntax_errors_fail_with_a_rendered_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bad.ts");
    std::fs::write(&file, "let x: = 1;\n").unwrap();
    let out = trioxide(&["transform", file.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("Unexpected token"),
        "{}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("bad.ts:1:8"), "{}", stderr(&out));
}

#[test]
fn build_writes_modules_and_sourcemaps() {
    let out_dir = tempfile::tempdir().unwrap();
    let dir = fixture("imports");
    let out = trioxide(&[
        "build",
        "--root",
        dir.to_str().unwrap(),
        "--out-dir",
        out_dir.path().to_str().unwrap(),
        dir.join("input.ts").to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    for name in ["input", "math", "shapes", "named"] {
        let js = std::fs::read_to_string(out_dir.path().join(format!("{name}.js"))).unwrap();
        assert!(
            js.ends_with(&format!("//# sourceMappingURL={name}.js.map\n")),
            "{js}"
        );
        let map = std::fs::read_to_string(out_dir.path().join(format!("{name}.js.map"))).unwrap();
        assert!(map.contains(&format!("{name}.ts")), "{map}");
    }
}

#[test]
fn config_files_are_read_and_validated() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.ts"), "export const a: number = 1;\n").unwrap();
    std::fs::write(
        dir.path().join("trioxide.jsonc"),
        "{ /* ok */ \"sourcemap\": false, }",
    )
    .unwrap();
    let file = dir.path().join("a.ts");
    let out = trioxide(&["transform", file.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "export const a = 1;\n");

    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "{ \"mdoe\": \"auto\" }").unwrap();
    let out = trioxide(&[
        "--config",
        bad.to_str().unwrap(),
        "transform",
        file.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("unknown field `mdoe`"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn transform_emit_rust_prints_the_generated_crate() {
    let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/wasm/recursion/input.ts");
    let out = trioxide(&["--emit=rust", "transform", input.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("// kernel/island.json"), "{stdout}");
    assert!(stdout.contains("// kernel/src/lib.rs"), "{stdout}");
    assert!(stdout.contains("fn fib("), "{stdout}");

    // Without directives there is nothing to print.
    let out = trioxide(&[
        "--emit=rust",
        "transform",
        fixture("classes/input.ts").to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(out.stdout.len(), 0);
}

#[test]
fn unimplemented_features_say_so() {
    let input = fixture("classes/input.ts");
    let input = input.to_str().unwrap();
    for args in [
        vec!["--emit=wasm", "transform", input],
        vec!["--emit=report", "transform", input],
    ] {
        let out = trioxide(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            stderr(&out).contains("not yet implemented"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
    // `explain` needs something to explain (the working directory has no index.html).
    let out = trioxide(&["explain"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("needs a file"), "{}", stderr(&out));
    // The partitioner uses these.
    for flag in ["--mode=wasm-preferred", "--mode=js-only"] {
        let out = trioxide(&[flag, "transform", input]);
        assert!(out.status.success(), "{flag}: {}", stderr(&out));
        assert!(
            !stderr(&out).contains("not yet implemented"),
            "{flag}: {}",
            stderr(&out)
        );
    }
    // The analysis uses these.
    for flag in ["--world=app", "--trust-types"] {
        let out = trioxide(&[flag, "transform", input]);
        assert!(out.status.success(), "{flag}: {}", stderr(&out));
        assert!(
            !stderr(&out).contains("not yet implemented"),
            "{flag}: {}",
            stderr(&out)
        );
    }
}

#[test]
fn emit_facts_prints_the_analysis() {
    let input = fixture("analysis/basics/input.ts");
    let out = trioxide(&["--emit=facts", "transform", input.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("functions:") && text.contains("nodes:"),
        "{text}"
    );
    assert!(text.contains("@proven"), "{text}");

    let out = trioxide(&["--emit=facts", "build", input.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

#[test]
fn check_toolchain_reports_what_is_missing() {
    let out = Command::new(env!("CARGO_BIN_EXE_trioxide"))
        .arg("check-toolchain")
        .env("PATH", "")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stderr(&out);
    assert!(text.contains("rustc      missing"), "{text}");
    assert!(
        text.contains("rustup target add") || text.contains("https://rustup.rs"),
        "{text}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_trioxide"))
        .args(["check-toolchain", "--json"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["canBuildIslands"], true, "{json}");
    assert_eq!(json["toolchain"]["wasm32Target"], true, "{json}");
    assert!(
        json["toolchain"]["rustc"]["version"]
            .as_str()
            .unwrap()
            .starts_with("rustc ")
    );
}

#[test]
fn config_is_discovered_above_the_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let app = dir.path().join("packages/app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("tsconfig.json"), "{}").unwrap();
    std::fs::write(app.join("a.ts"), "export const a: number = 1;\n").unwrap();
    std::fs::write(dir.path().join("trioxide.json"), r#"{ "mdoe": 1 }"#).unwrap();
    let file = app.join("a.ts");
    let out = trioxide(&["transform", file.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("unknown field `mdoe`"),
        "{}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains(dir.path().join("trioxide.json").to_str().unwrap()),
        "{}",
        stderr(&out)
    );
}

#[test]
fn diagnostics_are_coloured_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bad.ts");
    std::fs::write(&file, "let x: = 1;\n").unwrap();
    let file = file.to_str().unwrap();
    let coloured = trioxide(&["--color=always", "transform", file]);
    assert!(stderr(&coloured).contains("\x1b["), "{}", stderr(&coloured));
    let plain = trioxide(&["--color=never", "transform", file]);
    assert!(!stderr(&plain).contains("\x1b["), "{}", stderr(&plain));
    // Not a terminal, so `auto` is plain too.
    let auto = trioxide(&["transform", file]);
    assert!(!stderr(&auto).contains("\x1b["), "{}", stderr(&auto));
}

#[test]
fn build_watch_rebuilds_on_change() {
    use std::{
        io::{BufRead as _, BufReader},
        process::Stdio,
        sync::mpsc,
        time::Duration,
    };

    let dir = tempfile::tempdir().unwrap();
    let out_dir = dir.path().join("dist");
    std::fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
    std::fs::write(
        dir.path().join("main.ts"),
        "import { two } from './two.js';\nexport const four: number = two * 2;\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("two.ts"), "export const two: number = 2;\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_trioxide"))
        .args(["build", "--watch", "--out-dir"])
        .arg(&out_dir)
        .arg(dir.path().join("main.ts"))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (lines, receiver) = mpsc::channel();
    let stderr = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            if lines.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let wait_for_build = || loop {
        let line = receiver.recv_timeout(Duration::from_secs(60)).unwrap();
        if line.contains("watching for changes") {
            return line;
        }
    };

    let first = wait_for_build();
    assert!(first.contains("built 2 modules"), "{first}");
    assert_eq!(
        std::fs::read_to_string(out_dir.join("two.js")).unwrap(),
        "export const two = 2;\n//# sourceMappingURL=two.js.map\n"
    );

    std::fs::write(
        dir.path().join("two.ts"),
        "export const two: number = 22;\n",
    )
    .unwrap();
    wait_for_build();
    assert_eq!(
        std::fs::read_to_string(out_dir.join("two.js")).unwrap(),
        "export const two = 22;\n//# sourceMappingURL=two.js.map\n"
    );

    std::fs::write(dir.path().join("two.ts"), "export const two: = ;\n").unwrap();
    let failed = wait_for_build();
    assert!(!failed.contains("built"), "{failed}");

    child.kill().unwrap();
    child.wait().unwrap();
}
