//! Builds the hand-written islands in `tests/islands/` and checks the builder's cache,
//! toolchain fallback, `wasm-opt` handling, and the glue's behaviour in Node.
//!
//! Island builds run real cargo. Every cache lives under `CARGO_TARGET_TMPDIR`, inside the
//! repository, so the pinned `rust-toolchain.toml` (which has the wasm32 target) applies.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use trioxide_wasm::{
    BuildError, BuilderOptions, CargoIslandBuilder, GlueMode, GlueOptions, IslandBuilder,
    IslandSource, IslandSpec, WasmUrl, generate_glue, toolchain::Probe,
};

fn islands_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/islands")
}

/// Every hand-written island, by fixture name.
fn hand_written() -> Vec<(String, IslandSource)> {
    let mut islands: Vec<(String, IslandSource)> = std::fs::read_dir(islands_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|dir| dir.join("island/island.json").is_file())
        .map(|dir| {
            let name = dir.file_name().unwrap().to_string_lossy().into_owned();
            (name, IslandSource::load(&dir.join("island")).unwrap())
        })
        .collect();
    islands.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(islands.len() >= 7, "hand-written islands are missing");
    islands
}

/// A cache shared by tests that don't care whether it is warm.
fn shared_cache() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("island-cache")
}

fn fresh_cache(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn builder(cache_dir: PathBuf, path: Option<OsString>) -> CargoIslandBuilder {
    CargoIslandBuilder::new(BuilderOptions {
        path,
        ..BuilderOptions::new(cache_dir)
    })
}

/// The export names of a wasm module (just enough of the binary format to find them).
fn wasm_exports(wasm: &[u8]) -> Vec<String> {
    fn leb(bytes: &[u8], at: &mut usize) -> usize {
        let (mut value, mut shift) = (0, 0);
        loop {
            let byte = bytes[*at];
            *at += 1;
            value |= usize::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                return value;
            }
        }
    }
    assert_eq!(&wasm[..4], b"\0asm");
    let mut at = 8;
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let size = leb(wasm, &mut at);
        let end = at + size;
        if id == 7 {
            let count = leb(wasm, &mut at);
            return (0..count)
                .map(|_| {
                    let len = leb(wasm, &mut at);
                    let name = String::from_utf8(wasm[at..at + len].to_vec()).unwrap();
                    at += len + 1;
                    leb(wasm, &mut at);
                    name
                })
                .collect();
        }
        at = end;
    }
    Vec::new()
}

#[test]
fn glue_snapshots() {
    for (name, island) in hand_written() {
        island.validate().unwrap();
        for (mode, suffix) in [(GlueMode::Tla, "tla"), (GlueMode::Ready, "ready")] {
            let glue = generate_glue(
                &island.spec,
                &GlueOptions {
                    mode,
                    wasm_url: WasmUrl::Sibling,
                },
            );
            insta::assert_snapshot!(format!("{name}-{suffix}"), glue);
        }
    }
    let nbody = IslandSource::load(&islands_dir().join("nbody/island")).unwrap();
    let vite = generate_glue(
        &nbody.spec,
        &GlueOptions {
            mode: GlueMode::Tla,
            wasm_url: WasmUrl::Expression("import.meta.ROLLUP_FILE_URL_nbody".into()),
        },
    );
    assert!(vite.contains("new URL(import.meta.ROLLUP_FILE_URL_nbody, import.meta.url)"));
}

/// The specs in `tests/islands` round-trip and their struct layouts match what the
/// hand-written Rust asserts.
#[test]
fn specs_round_trip() {
    for (name, island) in hand_written() {
        let json = island.spec.to_json();
        assert_eq!(IslandSpec::from_json(&json).unwrap(), island.spec, "{name}");
    }
    let particles = IslandSource::load(&islands_dir().join("particles/island")).unwrap();
    let body = particles.spec.struct_layout(0);
    assert_eq!((body.size, body.align), (48, 8));
    assert_eq!(body.fields[5].offset, 40);
    assert_eq!(body.fields[6].offset, 44);
}

/// One cargo run builds every island; an unchanged rebuild is served from the cache without
/// invoking cargo at all, even when `cargo` on PATH would fail.
#[cfg(unix)]
#[test]
fn builds_all_islands_once_then_serves_the_cache() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let islands: Vec<IslandSource> = hand_written().into_iter().map(|(_, i)| i).collect();
    let cache = fresh_cache("cache-test");
    let first = builder(cache.clone(), None);
    let artifacts: Vec<_> = first
        .build_all(&islands)
        .into_iter()
        .map(|result| result.unwrap())
        .collect();
    assert_eq!(
        first.cargo_invocations(),
        1,
        "one cargo run for the whole batch"
    );
    for (island, artifact) in islands.iter().zip(&artifacts) {
        let exports = wasm_exports(&artifact.wasm);
        for name in [
            "memory",
            "__trioxide_alloc",
            "__trioxide_reset",
            "__trioxide_abi_version",
        ]
        .into_iter()
        .chain(island.spec.exports.iter().map(|e| e.wasm_name.as_str()))
        {
            assert!(
                exports.iter().any(|e| e == name),
                "{}: missing export {name} in {exports:?}",
                island.spec.id
            );
        }
        assert!(artifact.warnings.is_empty(), "{:?}", artifact.warnings);
        assert!(artifact.glue.contains(&artifact.wasm_file_name()));
    }

    // PATH with the real rustc but a cargo that records every call and fails builds.
    let bin = tempfile::tempdir().unwrap();
    let rustc = Probe::default().find("rustc").unwrap();
    symlink(&rustc, bin.path().join("rustc")).unwrap();
    let log = bin.path().join("cargo.log");
    let cargo = bin.path().join("cargo");
    std::fs::write(
        &cargo,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\n[ \"$1\" = --version ] && echo 'cargo 0.0.0 (fake)' && exit 0\nexit 1\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();

    let second = builder(cache, Some(bin.path().as_os_str().to_owned()));
    let cached: Vec<_> = second
        .build_all(&islands)
        .into_iter()
        .map(|result| result.unwrap())
        .collect();
    assert_eq!(second.cargo_invocations(), 0);
    assert!(
        !log.exists(),
        "cargo was invoked: {:?}",
        std::fs::read_to_string(&log)
    );
    assert_eq!(cached, artifacts);

    // Changing a source misses the cache, so the fake cargo runs (and fails).
    let mut changed = islands[0].clone();
    changed
        .files
        .get_mut("src/lib.rs")
        .unwrap()
        .push_str("\n// changed\n");
    let error = second.build(&changed).unwrap_err();
    assert!(matches!(error, BuildError::Cargo { .. }), "{error}");
    assert!(log.exists());
}

#[test]
fn missing_toolchain_is_reported_for_every_island() {
    let empty = tempfile::tempdir().unwrap();
    let builder = builder(
        fresh_cache("no-toolchain"),
        Some(empty.path().as_os_str().to_owned()),
    );
    let islands: Vec<IslandSource> = hand_written().into_iter().map(|(_, i)| i).collect();
    for result in builder.build_all(&islands) {
        match result {
            Err(BuildError::ToolchainUnavailable(reason)) => {
                assert!(reason.contains("rustc"), "{reason}");
            }
            other => panic!("expected ToolchainUnavailable, got {other:?}"),
        }
    }
    assert_eq!(builder.cargo_invocations(), 0);
}

#[test]
fn invalid_islands_are_rejected_before_building() {
    let (_, mut island) = hand_written().remove(0);
    island.spec.exports[0].wasm_name = "__trioxide_alloc".into();
    let error = builder(shared_cache(), None).build(&island).unwrap_err();
    assert!(matches!(error, BuildError::InvalidSpec(_)), "{error}");

    let (_, mut island) = hand_written().remove(0);
    island.files.insert("../escape.rs".into(), String::new());
    let error = builder(shared_cache(), None).build(&island).unwrap_err();
    assert!(error.to_string().contains("not under src/"), "{error}");
}

/// `wasm-opt` runs when found, with -O3; when it fails, the unoptimised module is used with
/// a warning.
#[cfg(unix)]
#[test]
fn runs_wasm_opt_when_present() {
    use std::os::unix::fs::PermissionsExt as _;

    let island = IslandSource::load(&islands_dir().join("nbody/island")).unwrap();
    for (succeed, expect_warning) in [(true, false), (false, true)] {
        let bin = tempfile::tempdir().unwrap();
        let log = bin.path().join("wasm-opt.log");
        let script = bin.path().join("wasm-opt");
        let action = if succeed {
            "cp \"$2\" \"$4\""
        } else {
            "exit 1"
        };
        std::fs::write(
            &script,
            format!(
                // A version unique to this run, so the module isn't already cached.
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'wasm-opt version 0 ({})'; exit 0; fi\necho \"$@\" >> '{}'\n{action}\n",
                bin.path().display(),
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(bin.path().to_path_buf())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();

        let artifact = builder(shared_cache(), Some(path)).build(&island).unwrap();
        let args = std::fs::read_to_string(&log).unwrap();
        assert!(args.starts_with("-O3 "), "{args}");
        assert_eq!(
            !artifact.warnings.is_empty(),
            expect_warning,
            "{:?}",
            artifact.warnings
        );
        assert_eq!(&artifact.wasm[..4], b"\0asm");
    }
}

/// Traps become errors that name the island, out-of-range lengths become `RangeError`s, the
/// island keeps working afterwards, and `ready()` glue refuses calls until instantiated.
#[test]
fn glue_reports_traps_and_waits_for_ready() {
    let spec = IslandSpec::from_json(
        r#"{
        "id": "traps", "abiVersion": 1,
        "exports": [
            { "name": "boom", "wasmName": "boom", "params": [{ "name": "x", "ty": { "kind": "f64" } }], "ret": { "kind": "f64" } },
            { "name": "make", "wasmName": "make", "params": [{ "name": "n", "ty": { "kind": "f64" } }], "ret": { "kind": "typedArray", "element": "f64" } }
        ]
    }"#,
    )
    .unwrap();
    let lib_rs = r#"#![no_std]
use trioxide_rt::prelude::*;

#[unsafe(export_name = "boom")]
pub extern "C" fn __abi_boom(x: f64) -> f64 {
    if x > 0.0 {
        core::arch::wasm32::unreachable();
    }
    x
}

#[unsafe(export_name = "make")]
pub extern "C" fn __abi_make(n: f64) -> u32 {
    abi::return_array(abi::new_array::<f64>(n))
}
"#;
    let island = IslandSource::new(spec, lib_rs.into());
    let artifact = builder(shared_cache(), None).build(&island).unwrap();

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("traps.wasm"), &artifact.wasm).unwrap();
    std::fs::write(dir.path().join("traps.js"), &artifact.glue).unwrap();
    let ready_glue = generate_glue(
        &island.spec,
        &GlueOptions {
            mode: GlueMode::Ready,
            wasm_url: WasmUrl::Sibling,
        },
    );
    std::fs::write(dir.path().join("traps-ready.mjs"), ready_glue).unwrap();
    std::fs::write(dir.path().join("package.json"), r#"{ "type": "module" }"#).unwrap();
    std::fs::write(
        dir.path().join("check.js"),
        r#"
const show = (f) => { try { return String(f()); } catch (e) { return `${e.constructor.name}: ${e.message} [${e.cause?.constructor.name}]`; } };
const { boom, make } = await import("./traps.js");
console.log(show(() => boom(1)));
console.log(show(() => boom(-2)));
console.log(show(() => make(-1)));
console.log(show(() => make(2 ** 40)));
console.log(show(() => make(2).length));
const ready = await import("./traps-ready.mjs");
console.log(show(() => ready.boom(-3)));
await ready.ready();
console.log(show(() => ready.boom(-3)));
"#,
    )
    .unwrap();
    let output = Command::new("node")
        .arg("check.js")
        .current_dir(dir.path())
        .output()
        .expect("node is needed for glue tests");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "Error: trioxide: island \"traps\" trapped in boom(); this is a trioxide bug [RuntimeError]",
            "-2",
            "RangeError: Invalid typed array length (in make(), island \"traps\") [undefined]",
            "RangeError: trioxide: island \"traps\" ran out of memory in make() [undefined]",
            "2",
            "Error: trioxide: island \"traps\" is not ready; await ready() first [undefined]",
            "-3",
        ]
    );
}
