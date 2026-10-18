//! Decision-log snapshots of the pass fixtures (`tests/fixtures/passes/**`): every module's
//! pass decisions, in order, with source positions, written to `decisions.snap` beside the
//! fixture. This is the data `trioxide explain` works from.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use trioxide::{Options, Project};
use trioxide_common::log::PassOutcome;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/passes")
        .canonicalize()
        .unwrap()
}

fn discover(dir: &Path, entries: &mut Vec<PathBuf>) {
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    children.sort();
    let entry = dir.join("input.ts");
    if entry.is_file() {
        entries.push(entry);
    }
    for child in children {
        if child.is_dir() {
            discover(&child, entries);
        }
    }
}

/// Line and column (1-based) of a byte offset.
fn position(source: &str, offset: u32) -> (usize, usize) {
    let before = &source[..offset as usize];
    let line = before.matches('\n').count() + 1;
    let column = before.len() - before.rfind('\n').map_or(0, |at| at + 1) + 1;
    (line, column)
}

#[test]
fn decisions() {
    let mut entries = Vec::new();
    discover(&fixtures_dir(), &mut entries);
    assert!(
        entries.len() > 10,
        "expected the pass fixtures, found {entries:?}"
    );
    for entry in entries {
        let dir = entry.parent().unwrap();
        let options = Options::find_config(dir)
            .map_or_else(Options::default, |path| Options::from_file(&path).unwrap());
        let mut project = Project::new(dir.to_path_buf(), options).unwrap();
        let output = project.build(std::slice::from_ref(&entry)).unwrap();
        let mut text = String::new();
        for module in &output.report.modules {
            let path = module
                .path
                .strip_prefix(project.root())
                .unwrap_or(&module.path);
            writeln!(text, "//- {}", path.display()).unwrap();
            let source = std::fs::read_to_string(&module.path).unwrap();
            for event in &module.log.passes {
                let at = event.span.map_or_else(String::new, |span| {
                    let (line, column) = position(&source, span.start);
                    format!("{line}:{column} ")
                });
                match &event.outcome {
                    PassOutcome::Applied => writeln!(text, "{at}{} applied", event.pass),
                    PassOutcome::Skipped { reason } => {
                        writeln!(text, "{at}{} skipped: {reason}", event.pass)
                    }
                }
                .unwrap();
            }
        }
        let name = dir
            .strip_prefix(fixtures_dir())
            .unwrap()
            .display()
            .to_string();
        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_path(dir);
        settings.set_prepend_module_to_snapshot(false);
        settings.set_input_file(&entry);
        settings.set_description(format!("tests/fixtures/passes/{name}"));
        settings.set_omit_expression(true);
        settings.bind(|| insta::assert_snapshot!("decisions", text));
    }
}

/// The default pipeline reads facts in one group (the foundation and speed passes) before any
/// rewrite, so building the pass fixtures never re-analyses the program to refresh them
/// (PLAN §3 "Refreshing facts"). A new fact-reading group after a rewrite would cost one
/// whole-program analysis per module it follows a change in; this test makes that visible.
#[test]
fn no_refreshes() {
    let mut entries = Vec::new();
    discover(&fixtures_dir(), &mut entries);
    let (mut count, mut time) = (0, std::time::Duration::ZERO);
    for entry in entries {
        let dir = entry.parent().unwrap();
        let options = Options::find_config(dir)
            .map_or_else(Options::default, |path| Options::from_file(&path).unwrap());
        let mut project = Project::new(dir.to_path_buf(), options).unwrap();
        project.build(std::slice::from_ref(&entry)).unwrap();
        let (refreshes, spent) = project.refresh_stats();
        count += refreshes;
        time += spent;
    }
    assert_eq!(
        count, 0,
        "the pass fixtures refreshed facts {count} times ({time:?})"
    );
}
