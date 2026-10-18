//! `trioxide explain` on `examples/mixed`, in both formats (snapshots in `snapshots/`).

use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
        .canonicalize()
        .unwrap()
}

/// Runs `trioxide explain` in `dir` and returns stdout with `dir` replaced by `<root>`.
fn explain(dir: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_trioxide"))
        .arg("explain")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    String::from_utf8(output.stdout)
        .unwrap()
        .replace(&dir.display().to_string(), "<root>")
}

#[test]
fn explains_the_mixed_example() {
    let dir = example("mixed");
    let text = explain(&dir, &["--entry", "src/main.ts"]);
    // The kernel becomes WASM; the boundary-heavy helper and the DOM code stay JS.
    assert!(
        text.contains("escapeTotal (6:8): WASM in island `kernel`"),
        "{text}"
    );
    assert!(text.contains("ease (5:8): JS"), "{text}");
    assert!(
        text.contains("uses the global `document` (a host API)"),
        "{text}"
    );
    insta::assert_snapshot!("explain_mixed", text);

    let json = explain(&dir, &["--json", "--entry", "src/main.ts", "src/kernel.ts"]);
    let report: serde_json::Value = serde_json::from_str(&json).unwrap();
    let functions = report["modules"][0]["functions"].as_array().unwrap();
    assert_eq!(functions[0]["target"], "wasm");
    assert!(functions[0]["cost"]["benefit"].as_f64().unwrap() > 0.0);
    insta::assert_snapshot!("explain_mixed_kernel_json", json);
}

#[test]
fn explains_one_function() {
    let dir = example("mixed");
    let text = explain(&dir, &["--entry", "src/main.ts", "src/view.ts:render"]);
    insta::assert_snapshot!("explain_mixed_render", text);
}
