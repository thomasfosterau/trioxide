//! Module graph discovery against real files.

use std::{fs, path::Path};

use trioxide_project::{
    CompilerSettings, GraphOptions, ImportKind, ImportTarget, JsxMode, ModuleGraph, find_tsconfig,
};

fn write(root: &Path, files: &[(&str, &str)]) {
    for (path, contents) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

fn names(graph: &ModuleGraph, root: &Path) -> Vec<String> {
    graph
        .modules()
        .iter()
        .map(|module| {
            module
                .path
                .strip_prefix(root)
                .unwrap()
                .display()
                .to_string()
        })
        .collect()
}

#[test]
fn follows_imports_with_tsconfig_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(
        &root,
        &[
            (
                "tsconfig.json",
                r#"{
                    // JSONC is fine
                    "compilerOptions": { "baseUrl": ".", "paths": { "@lib/*": ["src/lib/*"] }, "jsx": "react-jsx", "target": "ES2020" },
                }"#,
            ),
            (
                "src/main.ts",
                r#"import { util } from "@lib/util";
import type { Only } from "./types.js";
import data from "./data.json";
import { missing } from "./missing.js";
import pkg from "some-package";
export const lazy = () => import("./lazy.js");
console.log(util, data, missing, pkg);
"#,
            ),
            ("src/lib/util.ts", "export const util = 1;\n"),
            ("src/types.ts", "export type Only = string;\n"),
            ("src/data.json", "{}"),
            ("src/lazy.tsx", "export default <div />;\n"),
        ],
    );

    let tsconfig = find_tsconfig(&root.join("src")).unwrap();
    let settings = CompilerSettings::load(&tsconfig).unwrap();
    assert_eq!(settings.target.as_deref(), Some("es2020"));
    assert_eq!(settings.jsx, JsxMode::Automatic);
    assert!(!settings.use_define_for_class_fields);

    let mut graph = ModuleGraph::new(GraphOptions {
        root: root.clone(),
        tsconfig: Some(tsconfig),
    });
    let entries = graph.add_entries(&["src/main.ts".into()]).unwrap();
    assert_eq!(
        names(&graph, &root),
        ["src/main.ts", "src/lib/util.ts", "src/lazy.tsx"]
    );

    let main = graph.module(entries[0]);
    let targets: Vec<_> = main
        .imports
        .iter()
        .map(|import| (import.specifier.as_str(), &import.target, import.kind))
        .collect();
    assert_eq!(
        targets[0],
        (
            "@lib/util",
            &ImportTarget::Module(graph.get(&root.join("src/lib/util.ts")).unwrap()),
            ImportKind::Static
        )
    );
    assert!(matches!(
        targets[1],
        (
            "./data.json",
            ImportTarget::External { path: Some(_) },
            ImportKind::Static
        )
    ));
    assert_eq!(
        targets[2],
        (
            "./missing.js",
            &ImportTarget::Unresolved,
            ImportKind::Static
        )
    );
    assert_eq!(
        targets[3],
        (
            "some-package",
            &ImportTarget::External { path: None },
            ImportKind::Static
        )
    );
    assert!(matches!(
        targets[4],
        ("./lazy.js", ImportTarget::Module(_), ImportKind::Dynamic)
    ));
    assert_eq!(targets.len(), 5, "type-only imports are not followed");
    assert_eq!(main.diagnostics.len(), 1, "{:?}", main.diagnostics);
    assert_eq!(
        main.diagnostics[0].code.as_deref(),
        Some("trioxide(unresolved-import)")
    );
}

#[test]
fn updates_keep_ids_and_report_dependants() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(
        &root,
        &[
            ("a.ts", "import { b } from './b.js';\nexport const a = b;\n"),
            ("b.ts", "import { c } from './c.js';\nexport const b = c;\n"),
            ("c.ts", "export const c = 1;\n"),
        ],
    );
    let mut graph = ModuleGraph::new(GraphOptions {
        root: root.clone(),
        tsconfig: None,
    });
    graph.add_entries(&[root.join("a.ts")]).unwrap();
    let c = graph.get(&root.join("c.ts")).unwrap();
    let before = graph.module(c).hash;
    assert_eq!(graph.dependants(c).len(), 2);

    // An in-memory edit that adds a new import loads the new module.
    assert_eq!(
        graph.set_source(
            &root.join("c.ts"),
            Some("export { d as c } from './d.js';\n".into())
        ),
        Some(c)
    );
    assert_ne!(graph.module(c).hash, before);
    assert_eq!(graph.module(c).imports[0].target, ImportTarget::Unresolved);
    write(&root, &[("d.ts", "export const d = 2;\n")]);
    graph.set_source(
        &root.join("c.ts"),
        Some("export { d as c } from './d.js';\n".into()),
    );
    assert_eq!(names(&graph, &root), ["a.ts", "b.ts", "c.ts", "d.ts"]);

    // Clearing the override re-reads the file.
    graph.set_source(&root.join("c.ts"), None);
    assert_eq!(graph.module(c).hash, before);
    assert_eq!(
        graph.set_source(&root.join("unknown.ts"), Some(String::new())),
        None
    );
}
