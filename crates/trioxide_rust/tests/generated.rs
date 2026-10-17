//! Snapshots of what the Rust backend generates for directive-forced islands.
//!
//! - `tests/islands/*/wasm/` and `tests/wasm/*/`: the generated crate (`island.json` and
//!   `src/lib.rs`), in `generated.snap` beside the fixture. The differential suite runs the
//!   same fixtures as JS+WASM.
//! - `tests/wasm-errors/*/`: forced functions that break an eligibility rule, with the
//!   diagnostics in `diagnostics.snap`.
//!
//! Review changes with `cargo insta review`.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use trioxide::{ExplainQuery, Options, Project};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn options_for(dir: &Path) -> Options {
    Options::find_config(dir)
        .map_or_else(Options::default, |path| Options::from_file(&path).unwrap())
}

/// Every `<root>/<case>/<sub>input.ts`, sorted.
fn fixtures(root: &str, sub: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(repo().join(root))
        .unwrap()
        .map(|entry| entry.unwrap().path().join(sub))
        .filter(|dir| dir.join("input.ts").is_file())
        .collect();
    dirs.sort();
    dirs
}

/// The diagnostics of the last analysis, with labels as `line:column`.
fn render_diagnostics(project: &Project, root: &Path) -> String {
    let mut text = String::new();
    for diagnostic in project.explain(&ExplainQuery::default()).diagnostics {
        let file = diagnostic
            .file
            .as_deref()
            .map(|f| f.strip_prefix(root).unwrap_or(f));
        let source = diagnostic
            .file
            .as_deref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .unwrap_or_default();
        let severity = format!("{:?}", diagnostic.severity).to_lowercase();
        let code = diagnostic.code.as_deref().unwrap_or("");
        writeln!(text, "{severity}[{code}]: {}", diagnostic.message).unwrap();
        for label in &diagnostic.labels {
            let (line, column) = position(&source, label.span.start);
            let file = file.map_or_else(String::new, |f| f.display().to_string());
            let excerpt = source[label.span.start as usize..label.span.end as usize]
                .lines()
                .next()
                .unwrap_or("");
            writeln!(text, "  --> {file}:{line}:{column} `{excerpt}`").unwrap();
            if let Some(message) = &label.message {
                writeln!(text, "      {message}").unwrap();
            }
        }
        if let Some(help) = &diagnostic.help {
            writeln!(text, "  help: {help}").unwrap();
        }
    }
    text
}

fn position(source: &str, offset: u32) -> (usize, usize) {
    let before = &source[..(offset as usize).min(source.len())];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rfind('\n')
        .map_or(before.len(), |i| before.len() - i - 1)
        + 1;
    (line, column)
}

fn snapshot(dir: &Path, name: &str, description: String, text: &str) {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(dir);
    settings.set_prepend_module_to_snapshot(false);
    settings.set_input_file(dir.join("input.ts"));
    settings.set_description(description);
    settings.set_omit_expression(true);
    settings.bind(|| insta::assert_snapshot!(name.to_string(), text));
}

#[test]
fn generated_islands() {
    let repo = repo();
    let mut dirs = fixtures("tests/islands", "wasm");
    dirs.extend(fixtures("tests/wasm", ""));
    assert!(
        dirs.len() >= 19,
        "expected the island variants and lowering fixtures: {dirs:?}"
    );
    for dir in dirs {
        let mut project = Project::new(dir.clone(), options_for(&dir)).unwrap();
        let islands = project.rust_islands(&[dir.join("input.ts")]).unwrap();
        let name = dir.strip_prefix(&repo).unwrap().display().to_string();
        let diagnostics = render_diagnostics(&project, &dir);
        assert!(diagnostics.is_empty(), "{name}:\n{diagnostics}");
        assert!(!islands.is_empty(), "{name} generated no island");
        let mut text = String::new();
        for island in islands {
            writeln!(
                text,
                "//- {}/island.json\n{}",
                island.spec.id,
                island.spec.to_json()
            )
            .unwrap();
            for (path, source) in &island.files {
                writeln!(text, "//- {}/{path}\n{source}", island.spec.id).unwrap();
            }
        }
        snapshot(&dir, "generated", name, text.trim_end());
    }
}

#[test]
fn ineligible_functions() {
    let repo = repo();
    let dirs = fixtures("tests/wasm-errors", "");
    assert!(
        dirs.len() >= 20,
        "expected one fixture per eligibility rule: {dirs:?}"
    );
    for dir in dirs {
        let mut project = Project::new(dir.clone(), options_for(&dir)).unwrap();
        let islands = project.rust_islands(&[dir.join("input.ts")]).unwrap();
        let name = dir.strip_prefix(&repo).unwrap().display().to_string();
        let diagnostics = render_diagnostics(&project, &dir);
        assert!(
            islands.is_empty() || name.ends_with("use-js-callee"),
            "{name} generated an island"
        );
        assert!(
            diagnostics.contains("error["),
            "{name} reported no error:\n{diagnostics}"
        );
        snapshot(&dir, "diagnostics", name, diagnostics.trim_end());
    }
}
