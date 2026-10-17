//! The `Project` lifecycle: build, transform from cache, update, explain.

use std::path::Path;

use trioxide::{ExplainQuery, Options, Project, Severity};

fn write(root: &Path, path: &str, contents: &str) {
    std::fs::write(root.join(path), contents).unwrap();
}

#[test]
fn build_update_transform_explain() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(
        &root,
        "main.ts",
        "import { n } from './n.js';\nexport const twice: number = n * 2;\n",
    );
    write(&root, "n.ts", "export const n: number = 21;\n");

    let mut project = Project::new(root.clone(), Options::default()).unwrap();
    let build = project.build(&[root.join("main.ts")]).unwrap();
    assert!(!build.has_errors());
    assert_eq!(build.modules.len(), 2);
    assert_eq!(build.modules[1].code, "export const n = 21;\n");
    assert!(
        build.modules[0]
            .map
            .as_ref()
            .unwrap()
            .as_json()
            .contains("main.ts")
    );
    assert_eq!(
        build.report.type_oracle,
        if trioxide::HAS_TSRS { "tsrs" } else { "noop" }
    );

    // Unchanged modules come from the cache.
    assert_eq!(
        project.transform(&root.join("n.ts")).unwrap(),
        build.modules[1]
    );

    let invalidation = project
        .update(
            &root.join("n.ts"),
            Some("export const n: number = 1;\n".into()),
        )
        .unwrap();
    assert_eq!(
        invalidation.changed.as_deref(),
        Some(root.join("n.ts").as_path())
    );
    assert_eq!(invalidation.dependants, [root.join("main.ts")]);
    assert_eq!(
        project.transform(&root.join("n.ts")).unwrap().code,
        "export const n = 1;\n"
    );

    // A module outside the graph is loaded on demand.
    write(&root, "other.ts", "export let x = 1 as number;\n");
    assert_eq!(
        project.transform(&root.join("other.ts")).unwrap().code,
        "export let x = 1;\n"
    );

    let report = project.explain(&ExplainQuery {
        file: Some(root.join("main.ts")),
        function: None,
    });
    assert_eq!(report.modules.len(), 1);
}

#[test]
fn errors_are_diagnostics_not_failures() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(
        &root,
        "main.ts",
        "import './broken.js';\nimport './gone.js';\n",
    );
    write(&root, "broken.ts", "class {\n");

    let mut project = Project::new(root.clone(), Options::default()).unwrap();
    let build = project.build(&[root.join("main.ts")]).unwrap();
    assert!(build.has_errors());
    let main = &build.modules[0];
    assert_eq!(main.diagnostics[0].severity, Severity::Warning);
    assert_eq!(
        main.diagnostics[0].code.as_deref(),
        Some("trioxide(unresolved-import)")
    );
    let broken = &build.modules[1];
    assert!(broken.has_errors());
    assert_eq!(broken.code, "");

    assert!(project.build(&[root.join("missing.ts")]).is_err());
}

#[test]
fn tsconfig_target_and_override() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(
        &root,
        "tsconfig.json",
        r#"{ "compilerOptions": { "target": "es2019" } }"#,
    );
    write(
        &root,
        "a.ts",
        "export const f = (o?: { x: number }) => o?.x ?? 0;\n",
    );

    let mut project = Project::new(root.clone(), Options::default()).unwrap();
    let lowered = project.transform(&root.join("a.ts")).unwrap().code;
    assert!(
        !lowered.contains("?."),
        "es2019 has no optional chaining:\n{lowered}"
    );

    let options = Options {
        target: Some("esnext".into()),
        ..Options::default()
    };
    let mut project = Project::new(root.clone(), options).unwrap();
    assert_eq!(
        project.transform(&root.join("a.ts")).unwrap().code,
        "export const f = (o) => o?.x ?? 0;\n"
    );
}

#[test]
fn check_reports_type_errors() {
    if !trioxide::HAS_TSRS {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(
        &root,
        "main.ts",
        "import { n } from './n.js';\nexport const s: string = n;\n",
    );
    write(&root, "n.ts", "export const n: number = 1;\n");

    let options = Options {
        check: true,
        ..Options::default()
    };
    let mut project = Project::new(root.clone(), options).unwrap();
    let build = project.build(&[root.join("main.ts")]).unwrap();
    let errors: Vec<_> = build
        .report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.as_deref() == Some("TS2322"))
        .collect();
    assert_eq!(errors.len(), 1, "{:?}", build.report.diagnostics);
    assert_eq!(
        errors[0].file.as_deref(),
        Some(root.join("main.ts").as_path())
    );

    // Without `check`, type errors are not reported.
    let mut project = Project::new(root.clone(), Options::default()).unwrap();
    let build = project.build(&[root.join("main.ts")]).unwrap();
    assert!(!build.has_errors(), "{:?}", build.report.diagnostics);
}

/// Re-analysing regenerated sources gives the facts a fresh analysis of them gives: what a
/// pass manager relies on between pass groups.
#[test]
fn reanalysis_matches_a_fresh_analysis() {
    let files = [
        (
            "main.ts",
            "import { scale } from './lib.js';\nfunction twice(x: number): number { return scale(x, 2); }\nexport function main(): void {\n  let total = 0;\n  for (let i = 0; i < 4; i++) total += twice(i);\n  console.log(total);\n}\n",
        ),
        (
            "lib.ts",
            "export function scale(value: number, factor: number): number {\n  return (value * factor) | 0;\n}\n",
        ),
    ];
    // The "rewrite": printing each module back out, as a pass manager does between groups.
    let regenerate = |source: &str| {
        let allocator = oxc_allocator::Allocator::default();
        let parsed =
            oxc_parser::Parser::new(&allocator, source, oxc_span::SourceType::ts()).parse();
        oxc_codegen::Codegen::new().build(&parsed.program).code
    };
    let options = || Options {
        world: Some(trioxide::World::App),
        ..Options::default()
    };
    let facts_of = |project: &Project| {
        let mut types: Vec<String> = project
            .analysis()
            .node_types()
            .into_iter()
            .map(|(node, fact)| format!("{node:?} {fact:?}"))
            .collect();
        types.sort();
        types
    };

    let original = tempfile::tempdir().unwrap();
    let original = original.path().canonicalize().unwrap();
    for (name, source) in files {
        write(&original, name, source);
    }
    let mut project = Project::new(original.clone(), options()).unwrap();
    project.facts(&original.join("main.ts")).unwrap();
    let regenerated: Vec<(std::path::PathBuf, String)> = files
        .iter()
        .map(|(name, source)| (original.join(name), regenerate(source)))
        .collect();
    let reanalysed = {
        project.reanalyse(&regenerated);
        facts_of(&project)
    };

    let fresh = tempfile::tempdir().unwrap();
    let fresh = fresh.path().canonicalize().unwrap();
    for (path, source) in &regenerated {
        write(&fresh, path.file_name().unwrap().to_str().unwrap(), source);
    }
    let mut project = Project::new(fresh.clone(), options()).unwrap();
    project.facts(&fresh.join("main.ts")).unwrap();
    let fresh_facts = facts_of(&project);

    assert!(reanalysed.len() > 10, "{reanalysed:?}");
    assert_eq!(reanalysed, fresh_facts);
}

/// Refreshing facts through the pass manager after a real rewrite gives what a fresh
/// analysis of the rewritten program gives.
#[test]
fn refreshed_facts_match_a_fresh_analysis() {
    use oxc_ast::ast::{Expression, Statement};
    use trioxide_js::{FactSource, Pass, PassContext, PassGroup, PassManager, PassOptions};

    /// Doubles the `const k = <number>` initialiser, once.
    struct DoubleK(bool);

    impl Pass for DoubleK {
        fn name(&self) -> &'static str {
            "double-k"
        }

        fn run<'a>(
            &mut self,
            program: &mut oxc_ast::ast::Program<'a>,
            _ctx: &mut PassContext<'a, '_>,
        ) -> bool {
            if self.0 {
                return false;
            }
            let Some(Statement::VariableDeclaration(declaration)) = program.body.first_mut() else {
                return false;
            };
            let Some(Expression::NumericLiteral(literal)) = &mut declaration.declarations[0].init
            else {
                return false;
            };
            literal.value *= 2.0;
            literal.raw = None;
            self.0 = true;
            true
        }
    }

    let source = "const k = 21;\nfunction twice(x: number): number {\n  return x * 2;\n}\nexport function main(): void {\n  console.log(twice(k));\n}\n";
    let options = || Options {
        world: Some(trioxide::World::App),
        ..Options::default()
    };
    let facts_of = |facts: &trioxide_analysis::Facts| {
        let mut types: Vec<String> = facts
            .node_types()
            .into_iter()
            .map(|(node, fact)| format!("{node:?} {fact:?}"))
            .collect();
        types.sort();
        types
    };

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(&root, "main.ts", source);
    let mut project = Project::new(root.clone(), options()).unwrap();
    project.facts(&root.join("main.ts")).unwrap();
    let before = facts_of(project.analysis());
    let id = project.module_id(&root.join("main.ts")).unwrap();

    let allocator = oxc_allocator::Allocator::default();
    let mut program = oxc_parser::Parser::new(&allocator, source, oxc_span::SourceType::ts())
        .parse()
        .program;
    let scoping = trioxide_js::rebuild_scoping(&program);
    let mut manager = PassManager::new(PassOptions::default());
    manager.add_group(PassGroup {
        name: "test",
        passes: vec![Box::new(DoubleK(false))],
        fixpoint: true,
    });
    let mut refresher = project.fact_refresher();
    let mut log = trioxide::log::ModuleLog::default();
    manager.run(
        &allocator,
        &mut program,
        scoping,
        id,
        &mut refresher,
        &mut log,
    );
    assert_eq!(refresher.refreshes(), 1);
    let refreshed = facts_of(refresher.facts());
    drop(refresher);
    let rewritten = oxc_codegen::Codegen::new().build(&program).code;
    assert!(rewritten.contains("const k = 42"), "{rewritten}");

    let fresh_dir = tempfile::tempdir().unwrap();
    let fresh_root = fresh_dir.path().canonicalize().unwrap();
    write(&fresh_root, "main.ts", &rewritten);
    let mut fresh = Project::new(fresh_root.clone(), options()).unwrap();
    fresh.facts(&fresh_root.join("main.ts")).unwrap();

    assert_ne!(refreshed, before, "the rewrite changed what is known");
    assert_eq!(refreshed, facts_of(fresh.analysis()));
}
