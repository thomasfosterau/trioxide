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
