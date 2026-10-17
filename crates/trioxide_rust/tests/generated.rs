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

/// Pretends to build every island (no cargo), so tests can check how modules call them.
struct FakeBuilder;

impl trioxide::IslandBuilder for FakeBuilder {
    fn build(
        &self,
        island: &trioxide::IslandSource,
    ) -> Result<trioxide::IslandArtifact, trioxide::IslandBuildError> {
        Ok(trioxide::IslandArtifact {
            spec: island.spec.clone(),
            wasm: Vec::new(),
            glue: String::new(),
            warnings: Vec::new(),
        })
    }
}

fn build_with(dir: &Path, builder: Box<dyn trioxide::IslandBuilder>) -> trioxide::BuildOutput {
    let mut project = Project::new(dir.to_path_buf(), options_for(dir)).unwrap();
    project.set_island_builder(builder);
    project.build(&[dir.join("input.ts")]).unwrap()
}

fn module<'o>(output: &'o trioxide::BuildOutput, name: &str) -> &'o trioxide::ModuleOutput {
    output
        .modules
        .iter()
        .find(|m| m.path.file_name().is_some_and(|f| f == name))
        .unwrap()
}

#[test]
fn built_islands_take_over_direct_calls() {
    let dir = repo().join("tests/wasm/multi-function");
    let output = build_with(&dir, Box::new(FakeBuilder));
    assert!(!output.has_errors(), "{:?}", output.report.diagnostics);
    assert_eq!(output.islands.len(), 1);
    let input = module(&output, "input.ts");
    // The kernel is still imported first (evaluation order); island exports come from the
    // glue.
    assert!(
        input
            .code
            .contains("import { area, curve } from \"\\0trioxide:island/kernel\";"),
        "{}",
        input.code
    );
    // `describe` isn't forced: it still comes from the kernel. `lerp` stays JS too.
    assert!(
        input.code.contains("describe } from \"./kernel.js\""),
        "{}",
        input.code
    );
    assert_eq!(input.islands.len(), 1);
    // The kernel's own JS caller (`describe`) calls the island; island members keep calling
    // each other in JS, so the fallback stays pure JS.
    let kernel = module(&output, "kernel.ts");
    assert!(kernel.code.contains("area$wasm(a, b)"), "{}", kernel.code);
    assert!(kernel.code.contains("curve(a, b, out);"), "{}", kernel.code);
}

#[test]
fn without_a_toolchain_calls_stay_in_js() {
    let dir = repo().join("tests/wasm/multi-function");
    let output = build_with(&dir, Box::new(trioxide::wasm::UnavailableIslandBuilder));
    assert_eq!(output.islands.len(), 0);
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|d| d.code.as_deref() == Some("trioxide(wasm-toolchain)")),
        "{:?}",
        output.report.diagnostics
    );
    for module in &output.modules {
        assert!(!module.code.contains("trioxide:island"), "{}", module.code);
        assert_eq!(module.islands.len(), 0);
    }
}

#[test]
fn no_wasm_ignores_directives() {
    let dir = repo().join("tests/wasm-errors/closure");
    let mut options = options_for(&dir);
    options.wasm.enabled = false;
    let mut project = Project::new(dir.clone(), options).unwrap();
    let output = project.build(&[dir.join("input.ts")]).unwrap();
    assert!(!output.has_errors(), "{:?}", output.report.diagnostics);
}

#[test]
fn eligibility_and_clusters() {
    use trioxide_rust::{
        NoExtraFacts, SourceModule,
        eligibility::{Eligibility, check, clusters},
    };
    let dir = repo().join("tests/wasm/recursion");
    let mut project = Project::new(dir.clone(), options_for(&dir)).unwrap();
    project.build(&[dir.join("input.ts")]).unwrap();
    let modules: Vec<SourceModule<'_>> = project
        .graph()
        .modules()
        .iter()
        .map(|m| SourceModule {
            id: m.id,
            path: &m.path,
            source: &m.source,
            source_type: m.source_type,
        })
        .collect();
    let facts = project.analysis();
    let kernel = modules
        .iter()
        .find(|m| m.path.ends_with("kernel.ts"))
        .unwrap();
    let functions: Vec<_> = facts
        .call_graph()
        .units()
        .iter()
        .copied()
        .filter(|unit| unit.module == kernel.id)
        .filter(|unit| facts.function(*unit).is_some_and(|f| f.name.is_some()))
        .collect();
    let named = |name: &str| {
        functions
            .iter()
            .copied()
            .find(|f| facts.function(*f).unwrap().name.as_deref() == Some(name))
            .unwrap()
    };
    let (fib, is_even, is_odd, power) = (
        named("fib"),
        named("isEven"),
        named("isOdd"),
        named("power"),
    );
    for function in [fib, is_even, power] {
        assert_eq!(
            check(&modules, facts, &NoExtraFacts, function),
            Eligibility::Eligible
        );
    }
    // isEven and isOdd call each other; fib and power only themselves.
    let mut groups = clusters(facts, &[fib, is_even, is_odd, power]);
    groups.sort_by_key(Vec::len);
    assert_eq!(groups, vec![vec![fib], vec![power], vec![is_even, is_odd]]);
}
