//! Snapshot tests: the facts for every module of every fixture under
//! `tests/fixtures/analysis/*/input.ts`, in `facts.snap` beside it.

mod common;

use std::fmt::Write as _;

#[test]
fn facts_snapshots() {
    let fixtures = common::fixtures();
    assert_ne!(fixtures, [] as [std::path::PathBuf; 0]);
    let filter = std::env::var("FIXTURE").ok();
    for entry in fixtures {
        let dir = entry.parent().unwrap();
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        if filter
            .as_ref()
            .is_some_and(|filter| !name.contains(filter.as_str()))
        {
            continue;
        }
        let loaded = common::load(&entry);
        let inputs = loaded.inputs();
        let mut text = String::new();
        for diagnostic in &loaded.analysis.diagnostics {
            writeln!(text, "//! {diagnostic}").unwrap();
        }
        for input in &inputs {
            let path = input.path.strip_prefix(dir).unwrap_or(input.path);
            writeln!(text, "//- {}", path.display()).unwrap();
            text.push_str(&trioxide_analysis::render(
                &loaded.analysis.facts,
                input,
                &inputs,
            ));
            text.push('\n');
        }
        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_path(dir);
        settings.set_prepend_module_to_snapshot(false);
        settings.set_input_file(&entry);
        settings.set_description(format!("tests/fixtures/analysis/{name}"));
        settings.set_omit_expression(true);
        settings.bind(|| insta::assert_snapshot!("facts", text.trim_end()));
    }
}
