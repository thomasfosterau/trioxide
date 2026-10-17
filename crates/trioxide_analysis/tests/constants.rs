//! The declaration queries: `symbol_constant`, `enum_member` and `export_used`, on small
//! programs written to a temporary directory.

mod common;

use std::path::Path;

use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use trioxide_analysis::{Constant, Fact, Trust};
use trioxide_common::SymbolRef;

/// Writes `files` (the first is the entry) and analyses them.
fn analyse(files: &[(&str, &str)]) -> (tempfile::TempDir, common::Loaded) {
    let dir = tempfile::tempdir().unwrap();
    for (name, source) in files {
        std::fs::write(dir.path().join(name), source).unwrap();
    }
    let loaded = common::load(&dir.path().join(files[0].0));
    (dir, loaded)
}

/// The module-level binding `name` of the module `file`.
fn symbol(dir: &Path, loaded: &common::Loaded, file: &str, name: &str) -> SymbolRef {
    let path = dir.join(file).canonicalize().unwrap();
    let module = loaded
        .graph
        .modules()
        .iter()
        .find(|module| module.path == path)
        .unwrap();
    let allocator = Allocator::default();
    let program = Parser::new(&allocator, &module.source, module.source_type)
        .parse()
        .program;
    let scoping = SemanticBuilder::new()
        .build(&program)
        .semantic
        .into_scoping();
    SymbolRef {
        module: module.id,
        symbol: scoping.get_root_binding(name.into()).unwrap(),
    }
}

fn owned(fact: &Fact<Option<&Constant>>) -> (Option<Constant>, Trust) {
    (fact.value.cloned(), fact.trust)
}

#[test]
fn const_literals() {
    let (dir, loaded) = analyse(&[(
        "input.ts",
        "const s = \"text\" as const;\nconst n = -2;\nlet l = 1;\nconst { d } = { d: 1 };\n\
         const u = undefined;\nconst b = 10n;\nexport function main(): void { console.log(s, n, l, d, u, b); }\n",
    )]);
    let facts = &loaded.analysis.facts;
    let constant =
        |name| owned(&facts.symbol_constant(symbol(dir.path(), &loaded, "input.ts", name)));
    assert_eq!(
        constant("s"),
        (Some(Constant::String("text".into())), Trust::Proven)
    );
    assert_eq!(constant("n"), (Some(Constant::Number(-2.0)), Trust::Proven));
    assert_eq!(constant("u"), (Some(Constant::Undefined), Trust::Proven));
    assert_eq!(
        constant("b"),
        (Some(Constant::BigInt("10".into())), Trust::Proven)
    );
    // `let` and destructuring are not constants of this kind.
    assert_eq!(constant("l"), (None, Trust::Unknown));
    assert_eq!(constant("d"), (None, Trust::Unknown));
}

#[test]
fn enum_members() {
    let (dir, loaded) = analyse(&[
        (
            "input.ts",
            "import { Shared } from \"./shared.js\";\n\
             enum Sealed { A = 1, B = \"b\" }\n\
             enum AsValue { A = 1 }\n\
             enum Written { A = 1 }\n\
             (Written as any).A = 2;\n\
             const keys = Object.keys(AsValue);\n\
             enum Late { A = 1 }\n\
             export function main(): void {\n\
               console.log(Sealed.A, Sealed.B, AsValue.A, Written.A, Late.A, keys, Shared.X);\n\
             }\n",
        ),
        ("shared.ts", "export enum Shared { X = 3 }\n"),
    ]);
    let facts = &loaded.analysis.facts;
    let member = |file, name, member| {
        owned(&facts.enum_member(symbol(dir.path(), &loaded, file, name), member))
    };
    assert_eq!(
        member("input.ts", "Sealed", "A"),
        (Some(Constant::Number(1.0)), Trust::Proven)
    );
    assert_eq!(
        member("input.ts", "Sealed", "B"),
        (Some(Constant::String("b".into())), Trust::Proven)
    );
    // Used as a value: only TypeScript says the members are read-only.
    assert_eq!(
        member("input.ts", "AsValue", "A"),
        (Some(Constant::Number(1.0)), Trust::Declared)
    );
    // A member is written, through a cast.
    assert_eq!(member("input.ts", "Written", "A"), (None, Trust::Unknown));
    // Declared after code that runs: a read could come first.
    assert_eq!(member("input.ts", "Late", "A"), (None, Trust::Unknown));
    // Through the import binding, and at the enum itself.
    assert_eq!(
        member("input.ts", "Shared", "X"),
        (Some(Constant::Number(3.0)), Trust::Proven)
    );
    assert_eq!(
        member("shared.ts", "Shared", "X"),
        (Some(Constant::Number(3.0)), Trust::Proven)
    );
}

#[test]
fn enum_in_an_import_cycle() {
    let (dir, loaded) = analyse(&[
        (
            "input.ts",
            "import { read } from \"./b.js\";\nexport enum E { A = 1 }\n\
             export function main(): void { console.log(read()); }\n",
        ),
        (
            "b.ts",
            "import { E } from \"./input.js\";\nexport function read(): number { return E.A; }\n",
        ),
    ]);
    let facts = &loaded.analysis.facts;
    let enum_symbol = symbol(dir.path(), &loaded, "input.ts", "E");
    assert_eq!(
        owned(&facts.enum_member(enum_symbol, "A")),
        (None, Trust::Unknown)
    );
}

#[test]
fn unused_exports() {
    let (dir, loaded) = analyse(&[
        (
            "input.ts",
            "import { used } from \"./lib.js\";\nimport * as all from \"./open.js\";\n\
             export const entryExport = 1;\n\
             export function main(): void { console.log(used, all); }\n",
        ),
        (
            "lib.ts",
            "export const used = 1;\nexport const unused = 2;\n",
        ),
        ("open.ts", "export const viaNamespace = 3;\n"),
    ]);
    let facts = &loaded.analysis.facts;
    let used = |file, name| {
        let fact = facts.export_used(symbol(dir.path(), &loaded, file, name));
        (fact.value, fact.trust)
    };
    assert_eq!(used("lib.ts", "unused"), (false, Trust::Proven));
    assert_eq!(used("lib.ts", "used"), (true, Trust::Unknown));
    // The namespace object exposes every export.
    assert_eq!(used("open.ts", "viaNamespace"), (true, Trust::Unknown));
    // The host calls the entry's exports.
    assert_eq!(used("input.ts", "entryExport"), (true, Trust::Unknown));
}

#[test]
fn exports_are_open_in_the_library_world() {
    let (dir, loaded) = analyse(&[
        (
            "input.ts",
            "// world: library\nimport { used } from \"./lib.js\";\n\
             export function main(): void { console.log(used); }\n",
        ),
        (
            "lib.ts",
            "export const used = 1;\nexport const unused = 2;\n",
        ),
    ]);
    let fact = loaded
        .analysis
        .facts
        .export_used(symbol(dir.path(), &loaded, "lib.ts", "unused"));
    assert_eq!((fact.value, fact.trust), (true, Trust::Unknown));
}
