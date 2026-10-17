//! Snapshot tests over `tests/fixtures/**/input.ts{,x}`.
//!
//! Every directory with an `input.ts` or `input.tsx` is a fixture. It is built as a project
//! rooted at that directory, and every module of the output is written into `output.snap`
//! beside it. Review changes with `cargo insta review`.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use trioxide::{Options, Project};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .canonicalize()
        .unwrap()
}

/// Finds fixture entries, depth first and sorted so the order is stable.
fn discover(dir: &Path, entries: &mut Vec<PathBuf>) {
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    children.sort();
    if let Some(entry) = ["input.ts", "input.tsx"]
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
    {
        entries.push(entry);
    }
    for child in children {
        if child.is_dir() && child.file_name().is_some_and(|name| name != "node_modules") {
            discover(&child, entries);
        }
    }
}

fn options_for(dir: &Path) -> Options {
    Options::find_config(dir)
        .map_or_else(Options::default, |path| Options::from_file(&path).unwrap())
}

/// The diagnostic with its file relative to `root`, so snapshots don't depend on the checkout.
fn relative(root: &Path, diagnostic: &trioxide::Diagnostic) -> trioxide::Diagnostic {
    let mut diagnostic = diagnostic.clone();
    if let Some(file) = &diagnostic.file {
        diagnostic.file = Some(file.strip_prefix(root).unwrap_or(file).to_path_buf());
    }
    diagnostic
}

/// Renders a build: each module's path relative to the fixture, its code, then diagnostics.
fn render(root: &Path, output: &trioxide::BuildOutput) -> String {
    let mut text = String::new();
    for module in &output.modules {
        let path = module.path.strip_prefix(root).unwrap_or(&module.path);
        writeln!(text, "//- {}", path.display()).unwrap();
        text.push_str(&module.code);
        for diagnostic in &module.diagnostics {
            writeln!(text, "//! {}", relative(root, diagnostic)).unwrap();
        }
        text.push('\n');
    }
    for diagnostic in &output.report.diagnostics {
        writeln!(text, "//! {}", relative(root, diagnostic)).unwrap();
    }
    text.trim_end().to_string() + "\n"
}

#[test]
fn fixtures() {
    let fixtures = fixtures_dir();
    let mut entries = Vec::new();
    discover(&fixtures, &mut entries);
    assert!(
        entries.len() >= 10,
        "expected the seed fixtures, found {entries:?}"
    );

    for entry in entries {
        let dir = entry.parent().unwrap();
        let mut project = Project::new(dir.to_path_buf(), options_for(dir)).unwrap();
        let output = project.build(std::slice::from_ref(&entry)).unwrap();
        let name = dir.strip_prefix(&fixtures).unwrap().display().to_string();
        assert!(
            !output.has_errors(),
            "fixture {name} has errors:\n{}",
            render(project.root(), &output)
        );

        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_path(dir);
        settings.set_prepend_module_to_snapshot(false);
        settings.set_input_file(&entry);
        settings.set_description(format!("tests/fixtures/{name}"));
        settings.set_omit_expression(true);
        settings.bind(|| insta::assert_snapshot!("output", render(project.root(), &output)));
    }
}
