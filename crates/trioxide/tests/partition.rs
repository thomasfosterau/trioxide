//! Automatic partitioning through the facade: modes, config overrides and directives, and
//! automatic islands going through the same call rewriting and `Keep` as forced ones.

use std::path::{Path, PathBuf};

use trioxide::{
    Mode, ModuleOutput, Options, Override, OverrideTarget, Project,
    log::{FunctionDecision, Target},
};

/// Pretends to build every island (no cargo), so tests see how modules call them.
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

fn mixed() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/mixed")
        .canonicalize()
        .unwrap()
}

fn build(options: Options) -> (trioxide::BuildOutput, Project) {
    let root = mixed();
    let mut project = Project::new(root.clone(), options).unwrap();
    project.set_island_builder(Box::new(FakeBuilder));
    let output = project.build(&[root.join("src/main.ts")]).unwrap();
    assert!(!output.has_errors(), "{:?}", output.report.diagnostics);
    (output, project)
}

fn module<'o>(output: &'o trioxide::BuildOutput, name: &str) -> &'o ModuleOutput {
    output
        .modules
        .iter()
        .find(|m| m.path.ends_with(name))
        .unwrap()
}

fn decision<'r>(report: &'r trioxide::Report, name: &str) -> &'r FunctionDecision {
    report
        .modules
        .iter()
        .flat_map(|m| &m.log.functions)
        .find(|f| f.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no decision for {name}"))
}

#[test]
fn auto_islands_take_over_calls_and_keep_their_exports() {
    let (output, _) = build(Options::default());
    assert_eq!(output.islands.len(), 1);
    assert_eq!(output.islands[0].spec.id.0, "kernel");
    let main = module(&output, "src/main.ts");
    assert!(
        main.code
            .contains("import { escapeTotal } from \"\\0trioxide:island/kernel\";"),
        "{}",
        main.code
    );
    // `ease` stays JS.
    assert!(
        main.code.contains("import { ease } from \"./ease.js\";"),
        "{}",
        main.code
    );
    // The glue's ready() fallback imports the kernel module's JS versions: they stay exported.
    let kernel = module(&output, "src/kernel.ts");
    assert!(
        kernel.code.contains("export function escapeTotal("),
        "{}",
        kernel.code
    );
    assert!(
        kernel.code.contains("function escapeTime("),
        "{}",
        kernel.code
    );
    let total = decision(&output.report, "escapeTotal");
    assert_eq!(total.target, Target::Wasm);
    assert_eq!(total.directive, None);
    let cost = total.cost.unwrap();
    assert!(cost.profitable(), "{cost:?}");
    let ease = decision(&output.report, "ease");
    assert_eq!(ease.target, Target::Js);
    assert!(!ease.cost.unwrap().profitable());
}

#[test]
fn js_only_and_no_wasm_create_no_islands() {
    let (output, _) = build(Options {
        mode: Mode::JsOnly,
        ..Options::default()
    });
    assert_eq!(output.islands.len(), 0);
    let mut options = Options::default();
    options.wasm.enabled = false;
    let (output, _) = build(options);
    assert_eq!(output.islands.len(), 0);
    for module in &output.modules {
        assert!(!module.code.contains("trioxide:island"), "{}", module.code);
    }
}

#[test]
fn wasm_preferred_lowers_the_threshold() {
    let mut options = Options {
        mode: Mode::WasmPreferred,
        ..Options::default()
    };
    options.cost_model.wasm_preferred_threshold = -10_000_000.0;
    let (output, _) = build(options);
    let ease = decision(&output.report, "ease");
    assert_eq!(ease.target, Target::Wasm, "{ease:?}");
    assert_eq!(ease.cost.unwrap().threshold, -10_000_000.0);
    assert_eq!(output.islands.len(), 2);
}

#[test]
fn config_overrides_force_and_forbid_whole_files() {
    let (output, _) = build(Options {
        overrides: vec![
            Override {
                files: vec!["src/kernel.ts".into()],
                target: OverrideTarget::Js,
            },
            Override {
                files: vec!["src/ease*.ts".into()],
                target: OverrideTarget::Wasm,
            },
        ],
        ..Options::default()
    });
    let total = decision(&output.report, "escapeTotal");
    assert_eq!(total.target, Target::Js);
    assert_eq!(total.directive, Some(Target::Js));
    assert_eq!(total.reason, "a config override keeps it in JS");
    let ease = decision(&output.report, "ease");
    assert_eq!(ease.target, Target::Wasm);
    assert_eq!(ease.directive, Some(Target::Wasm));
    assert_eq!(output.islands.len(), 1);
    assert_eq!(output.islands[0].spec.id.0, "ease");
    // Overrides apply in js-only mode too.
    let (output, _) = build(Options {
        mode: Mode::JsOnly,
        overrides: vec![Override {
            files: vec!["**/ease.ts".into()],
            target: OverrideTarget::Wasm,
        }],
        ..Options::default()
    });
    assert_eq!(output.islands.len(), 1);
}

#[test]
fn source_directives_win_over_the_cost_model() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for file in [
        "src/main.ts",
        "src/kernel.ts",
        "src/ease.ts",
        "src/view.ts",
        "tsconfig.json",
    ] {
        let from = mixed().join(file);
        let to = root.join(file);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        let mut text = std::fs::read_to_string(from).unwrap();
        if file == "src/kernel.ts" {
            text = text.replace(
                "): number {\n  let total = 0;",
                "): number {\n  \"use js\";\n  let total = 0;",
            );
        }
        if file == "src/ease.ts" {
            text = text.replace("): number {\n", "): number {\n  \"use wasm\";\n");
        }
        std::fs::write(to, text).unwrap();
    }
    let mut project = Project::new(root.to_path_buf(), Options::default()).unwrap();
    project.set_island_builder(Box::new(FakeBuilder));
    let output = project.build(&[root.join("src/main.ts")]).unwrap();
    assert!(!output.has_errors(), "{:?}", output.report.diagnostics);
    let total = decision(&output.report, "escapeTotal");
    assert_eq!(
        (total.target, total.directive),
        (Target::Js, Some(Target::Js))
    );
    let ease = decision(&output.report, "ease");
    assert_eq!(
        (ease.target, ease.directive),
        (Target::Wasm, Some(Target::Wasm))
    );
    assert_eq!(
        output
            .islands
            .iter()
            .map(|i| i.spec.id.0.as_str())
            .collect::<Vec<_>>(),
        ["ease"]
    );
}

#[test]
fn plan_explains_without_building() {
    let root = mixed();
    let mut project = Project::new(root.clone(), Options::default()).unwrap();
    // Building would fail: plan must not build.
    project.set_island_builder(Box::new(trioxide::wasm::UnavailableIslandBuilder));
    let report = project.plan(&[root.join("src/main.ts")]).unwrap();
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert_eq!(decision(&report, "escapeTotal").target, Target::Wasm);
    let narrowed = project.explain(&"src/kernel.ts:escapeTime".parse().unwrap());
    let names: Vec<_> = narrowed
        .modules
        .iter()
        .flat_map(|m| &m.log.functions)
        .filter_map(|f| f.name.as_deref())
        .collect();
    assert_eq!(names, ["escapeTime"]);
}
