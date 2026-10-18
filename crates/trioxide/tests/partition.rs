//! Automatic partitioning through the facade: modes, config overrides and directives, and
//! automatic islands going through the same call rewriting and `Keep` as forced ones.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

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
            .contains("import { tileDigest } from \"\\0trioxide:island/kernel\";"),
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
        kernel.code.contains("export function tileDigest("),
        "{}",
        kernel.code
    );
    assert!(
        kernel.code.contains("function cellNoise("),
        "{}",
        kernel.code
    );
    let total = decision(&output.report, "tileDigest");
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
    let total = decision(&output.report, "tileDigest");
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
                "export function tileDigest(seed: number): number {\n",
                "export function tileDigest(seed: number): number {\n  \"use js\";\n",
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
    let total = decision(&output.report, "tileDigest");
    assert_eq!(
        (total.target, total.directive),
        (Target::Js, Some(Target::Js))
    );
    let ease = decision(&output.report, "ease");
    assert_eq!(
        (ease.target, ease.directive),
        (Target::Wasm, Some(Target::Wasm))
    );
    // `cellNoise` is still a candidate of its own (its forbidden caller calls it from JS).
    assert!(output.islands.iter().any(|i| i.spec.id.0 == "ease"));
}

#[test]
fn plan_explains_without_building() {
    let root = mixed();
    let mut project = Project::new(root.clone(), Options::default()).unwrap();
    // Building would fail: plan must not build.
    project.set_island_builder(Box::new(trioxide::wasm::UnavailableIslandBuilder));
    let report = project.plan(&[root.join("src/main.ts")]).unwrap();
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert_eq!(decision(&report, "tileDigest").target, Target::Wasm);
    let narrowed = project.explain(&"src/kernel.ts:cellNoise".parse().unwrap());
    let names: Vec<_> = narrowed
        .modules
        .iter()
        .flat_map(|m| &m.log.functions)
        .filter_map(|f| f.name.as_deref())
        .collect();
    assert_eq!(names, ["cellNoise"]);
}

/// A project in a temporary directory with `files`.
fn project_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Project) {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    std::fs::copy(
        mixed().join("tsconfig.json"),
        dir.path().join("tsconfig.json"),
    )
    .unwrap();
    let mut project = Project::new(dir.path().to_path_buf(), Options::default()).unwrap();
    project.set_island_builder(Box::new(FakeBuilder));
    (dir, project)
}

/// An int32 kernel the cost model likes, under the name `name`, `export`ed or not.
fn kernel(name: &str, export: &str) -> String {
    format!(
        "{export}function {name}(seed: number): number {{
  let h = seed | 0;
  for (let i = 0; i < 4000000; i++) h = (h + Math.imul(h, 1597334677)) | 0;
  return h;
}}
"
    )
}

#[test]
fn only_calls_the_rewrite_can_redirect_count() {
    // A module-private kernel: `ready()` glue can't reach it, so it isn't chosen.
    let private = format!(
        "{}export function main(): void {{ console.log(mix(7)); }}\n",
        kernel("mix", "")
    );
    let (dir, mut project) = project_with(&[("input.ts", &private)]);
    let output = project.build(&[dir.path().join("input.ts")]).unwrap();
    assert_eq!(output.islands.len(), 0);
    let mix = decision(&output.report, "mix");
    assert_eq!(mix.target, Target::Js);
    assert!(
        mix.reason.contains("no call can reach an island"),
        "{mix:?}"
    );

    // Exported but only called by the host (an entry export): its JS version runs.
    let (dir, mut project) = project_with(&[("input.ts", &kernel("mix", "export "))]);
    let output = project.build(&[dir.path().join("input.ts")]).unwrap();
    assert_eq!(output.islands.len(), 0);

    // Exported and called from an importing module: chosen, and the call goes to the glue.
    let (dir, mut project) = project_with(&[
        ("kernel.ts", &kernel("mix", "export ")),
        (
            "input.ts",
            "import { mix } from \"./kernel.js\";\nexport function main(): void { console.log(mix(7)); }\n",
        ),
    ]);
    let output = project.build(&[dir.path().join("input.ts")]).unwrap();
    assert_eq!(output.islands.len(), 1);
    assert_eq!(decision(&output.report, "mix").target, Target::Wasm);
    assert!(
        module(&output, "input.ts")
            .code
            .contains("trioxide:island/kernel"),
        "{}",
        module(&output, "input.ts").code
    );
}

#[test]
fn islands_that_do_not_build_are_reported_as_js() {
    let root = mixed();
    let mut project = Project::new(root.clone(), Options::default()).unwrap();
    project.set_island_builder(Box::new(trioxide::wasm::UnavailableIslandBuilder));
    let output = project.build(&[root.join("src/main.ts")]).unwrap();
    assert_eq!(output.islands.len(), 0);
    let digest = decision(&output.report, "tileDigest");
    assert_eq!(digest.target, Target::Js);
    assert!(digest.reason.contains("wasn't built"), "{digest:?}");
}

#[test]
fn deep_loop_nests_saturate() {
    // Each function loops 2^24 times and calls the next: the product overflows f64 unless
    // the estimates saturate.
    let mut text = String::new();
    for i in 0..44 {
        let call = if i == 43 {
            String::new()
        } else {
            format!("s = (s + f{}(i)) | 0;", i + 1)
        };
        writeln!(
            text,
            "export function f{i}(x: number): number {{\n  let s = x | 0;\n  for (let i = 0; i < 16777215; i++) {{ {call} }}\n  return s;\n}}"
        )
        .unwrap();
    }
    let (dir, mut project) = project_with(&[
        ("kernel.ts", &text),
        (
            "input.ts",
            "import { f0 } from \"./kernel.js\";\nexport function main(): void { console.log(f0(1)); }\n",
        ),
    ]);
    let report = project.plan(&[dir.path().join("input.ts")]).unwrap();
    let cost = decision(&report, "f0").cost.unwrap();
    for value in [cost.benefit, cost.cost, cost.work, cost.calls] {
        assert!(value.is_finite(), "{cost:?}");
    }
    let json = serde_json::to_string(&report).unwrap();
    let back: trioxide::Report = serde_json::from_str(&json).unwrap();
    assert_eq!(back, report);
}
