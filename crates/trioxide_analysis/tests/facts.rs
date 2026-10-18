//! Snapshot tests: the facts for every module of every fixture under
//! `tests/fixtures/analysis/*/input.ts`, in `facts.snap` beside it; and the parallel
//! interpreter's promise that facts never depend on how many threads ran it.

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
        let text = rendered(&common::load(&entry));
        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_path(dir);
        settings.set_prepend_module_to_snapshot(false);
        settings.set_input_file(&entry);
        settings.set_description(format!("tests/fixtures/analysis/{name}"));
        settings.set_omit_expression(true);
        settings.bind(|| insta::assert_snapshot!("facts", text.trim_end()));
    }
}

/// Every module's facts, as the snapshots show them.
fn rendered(loaded: &common::Loaded) -> String {
    let inputs = loaded.inputs();
    let dir = loaded.graph.modules()[0].path.parent().unwrap();
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
    text
}

/// Small chunks, so most fixtures run several chunks per sweep: one thread and four must
/// agree exactly.
#[test]
fn facts_do_not_depend_on_threads() {
    for entry in common::fixtures() {
        let one = rendered(&common::load_with(&entry, 2, 1, 0));
        let four = rendered(&common::load_with(&entry, 2, 4, 0));
        assert_eq!(one, four, "{}", entry.display());
        // Giving entries up happens in the merge, so it doesn't depend on threads either.
        let one = rendered(&common::load_with(&entry, 2, 1, 1));
        let four = rendered(&common::load_with(&entry, 2, 4, 1));
        assert_eq!(one, four, "{} (giving up)", entry.display());
    }
}

/// The declaration facts (`symbol_constant`, `export_used`, `enum_member`) of every symbol of
/// every module of `loaded`, in order.
fn declaration_facts(loaded: &common::Loaded) -> String {
    use oxc_syntax::symbol::SymbolFlags;
    let facts = &loaded.analysis.facts;
    let mut text = String::new();
    for module in loaded.graph.modules() {
        let allocator = oxc_allocator::Allocator::default();
        let program = oxc_parser::Parser::new(&allocator, &module.source, module.source_type)
            .parse()
            .program;
        let scoping = oxc_semantic::SemanticBuilder::new()
            .build(&program)
            .semantic
            .into_scoping();
        let symbol = |id| trioxide_common::SymbolRef {
            module: module.id,
            symbol: id,
        };
        let members: Vec<&str> = scoping
            .symbol_ids()
            .filter(|&id| scoping.symbol_flags(id).contains(SymbolFlags::EnumMember))
            .map(|id| scoping.symbol_name(id))
            .collect();
        for id in scoping.symbol_ids() {
            let name = scoping.symbol_name(id);
            writeln!(
                text,
                "{} {name}: {:?} {:?}",
                module.path.display(),
                facts.symbol_constant(symbol(id)),
                facts.export_used(symbol(id)),
            )
            .unwrap();
            if scoping.symbol_flags(id).contains(SymbolFlags::RegularEnum) {
                for member in &members {
                    writeln!(
                        text,
                        "  {member}: {:?}",
                        facts.enum_member(symbol(id), member)
                    )
                    .unwrap();
                }
            }
        }
    }
    text
}

/// The declaration facts follow from declarations and references alone: no chunk size or
/// thread count changes them.
#[test]
fn declaration_facts_do_not_depend_on_chunking() {
    let mut known = 0;
    for entry in common::all_fixtures() {
        let expected = declaration_facts(&common::load(&entry));
        known += expected.matches("Some(").count();
        for (chunk_units, threads) in [(1, 1), (2, 4), (0, 4)] {
            let actual = declaration_facts(&common::load_with(&entry, chunk_units, threads, 0));
            assert_eq!(
                expected,
                actual,
                "{} (chunks of {chunk_units}, {threads} threads)",
                entry.display()
            );
        }
    }
    assert!(known > 0, "no fixture has a declaration fact");
}
