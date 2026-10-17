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
fn unimplemented_features_say_so() {
    let input = fixture("classes/input.ts");
    let input = input.to_str().unwrap();
    for args in [
        vec!["explain"],
        vec!["explain", "a.ts:f"],
        vec!["check-toolchain"],
        vec!["--emit=rust", "transform", input],
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
    for flag in [
        "--mode=wasm-preferred",
        "--world=app",
        "--trust-types",
        "--no-wasm",
    ] {
        let out = trioxide(&[flag, "transform", input]);
        assert!(out.status.success(), "{flag}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("not yet implemented"),
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
