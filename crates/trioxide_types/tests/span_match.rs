//! How well oxc spans line up with tsrs nodes (PLAN §3 "Span correlation").
//!
//! Parses a corpus (this crate's `tests/corpus` plus every fixture under `tests/fixtures`) with
//! oxc, asks the tsrs oracle for a node at the span of every identifier, member expression and
//! call, and reports the match rate per category. Each must be at least 99%.

#![cfg(feature = "tsrs")]
#![expect(clippy::print_stderr, reason = "the test reports its match rates")]

use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingIdentifier, CallExpression, ComputedMemberExpression, IdentifierName,
    IdentifierReference, NewExpression, PrivateFieldExpression, StaticMemberExpression,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use trioxide_common::ModuleId;
use trioxide_types::{OracleInput, SourceText, TsrsOracle, TypeOracle};

#[derive(Default)]
struct Tally {
    matched: usize,
    total: usize,
    misses: Vec<String>,
}

impl Tally {
    fn rate(&self) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        #[expect(clippy::cast_precision_loss, reason = "counts are small")]
        let rate = self.matched as f64 / self.total as f64;
        rate
    }
}

struct Counter<'o, 's> {
    oracle: &'o mut TsrsOracle,
    module: ModuleId,
    source: &'s str,
    path: &'s Path,
    identifiers: Tally,
    members: Tally,
    calls: Tally,
}

impl Counter<'_, '_> {
    fn check(&mut self, kind: Kind, span: Span) {
        let matched = self.oracle.matches_node(self.module, span);
        let tally = match kind {
            Kind::Identifier => &mut self.identifiers,
            Kind::Member => &mut self.members,
            Kind::Call => &mut self.calls,
        };
        tally.total += 1;
        if matched {
            tally.matched += 1;
        } else if tally.misses.len() < 20 {
            let line = self.source[..span.start as usize].lines().count();
            tally.misses.push(format!(
                "{}:{line}: `{}`",
                self.path.display(),
                span.source_text(self.source)
            ));
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Identifier,
    Member,
    Call,
}

impl<'a> Visit<'a> for Counter<'_, '_> {
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        self.check(Kind::Identifier, it.span);
    }

    fn visit_binding_identifier(&mut self, it: &BindingIdentifier<'a>) {
        self.check(Kind::Identifier, it.span);
    }

    fn visit_identifier_name(&mut self, it: &IdentifierName<'a>) {
        self.check(Kind::Identifier, it.span);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        self.check(Kind::Member, it.span);
        walk::walk_static_member_expression(self, it);
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        self.check(Kind::Member, it.span);
        walk::walk_computed_member_expression(self, it);
    }

    fn visit_private_field_expression(&mut self, it: &PrivateFieldExpression<'a>) {
        self.check(Kind::Member, it.span);
        walk::walk_private_field_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.check(Kind::Call, it.span);
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        self.check(Kind::Call, it.span());
        walk::walk_new_expression(self, it);
    }
}

fn corpus() -> Vec<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect(&manifest.join("tests/corpus"), &mut files);
    collect(&manifest.join("../../tests/fixtures"), &mut files);
    files.sort();
    files
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, files);
        } else if path
            .extension()
            .is_some_and(|ext| ext == "ts" || ext == "tsx")
            && !path.to_string_lossy().ends_with(".d.ts")
        {
            files.push(path.canonicalize().unwrap());
        }
    }
}

#[test]
fn match_rate_is_at_least_99_percent() {
    let files = corpus();
    let sources: Vec<String> = files
        .iter()
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect();
    let modules: Vec<SourceText<'_>> = files
        .iter()
        .zip(&sources)
        .enumerate()
        .map(|(index, (path, source))| SourceText {
            module: ModuleId::new(u32::try_from(index).unwrap()),
            path,
            source,
        })
        .collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut oracle = TsrsOracle::default();
    oracle.load(&OracleInput {
        root: &root,
        tsconfig: None,
        modules: &modules,
        check: false,
    });

    let (mut identifiers, mut members, mut calls) =
        (Tally::default(), Tally::default(), Tally::default());
    for module in &modules {
        let allocator = Allocator::default();
        let source_type = SourceType::from_path(module.path).unwrap();
        let parsed = Parser::new(&allocator, module.source, source_type).parse();
        assert!(
            parsed.diagnostics.is_empty(),
            "{} has parse errors",
            module.path.display()
        );
        let mut counter = Counter {
            oracle: &mut oracle,
            module: module.module,
            source: module.source,
            path: module.path,
            identifiers: Tally::default(),
            members: Tally::default(),
            calls: Tally::default(),
        };
        counter.visit_program(&parsed.program);
        for (total, part) in [
            (&mut identifiers, counter.identifiers),
            (&mut members, counter.members),
            (&mut calls, counter.calls),
        ] {
            total.matched += part.matched;
            total.total += part.total;
            total.misses.extend(part.misses);
        }
    }

    let mut failed = false;
    for (name, tally) in [
        ("identifiers", &identifiers),
        ("member expressions", &members),
        ("calls", &calls),
    ] {
        eprintln!(
            "span match rate for {name}: {}/{} = {:.2}%",
            tally.matched,
            tally.total,
            tally.rate() * 100.0
        );
        for miss in tally.misses.iter().take(10) {
            eprintln!("  unmatched: {miss}");
        }
        failed |= tally.rate() < 0.99 || tally.total == 0;
    }
    assert!(!failed, "a span match rate is below 99%");
}
