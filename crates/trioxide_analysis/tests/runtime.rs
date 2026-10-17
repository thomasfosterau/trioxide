//! Checks `Proven` facts against reality: every analysis fixture with a `main()` is
//! instrumented so that each expression with a proven type reports its runtime value to a
//! checker, then run in Node. Any value outside its proven type fails the test.
//!
//! Skipped (with a message) when `node` is not on PATH.

#![expect(clippy::print_stderr, reason = "the test reports what it skips")]

mod common;

use std::{fmt::Write as _, path::Path, process::Command};

use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_parser::Parser;
use oxc_semantic::{AstNodes, SemanticBuilder};
use oxc_span::{GetSpan, Span};
use trioxide_analysis::{Facts, Trust};
use trioxide_common::{ModuleId, NodeId, NodeRef};
use trioxide_types::Type;

/// A JS expression over `v` that holds when `v` is a value of `ty`.
fn predicate(ty: &Type) -> String {
    match ty {
        Type::Unknown => "true".into(),
        Type::Never => "false".into(),
        Type::Number(n) => {
            let mut parts = vec!["typeof v === 'number'".to_string()];
            if n.non_nan {
                parts.push("!Number.isNaN(v)".into());
            }
            let bound = |value: f64| {
                if value == f64::INFINITY {
                    "Infinity".to_string()
                } else if value == f64::NEG_INFINITY {
                    "-Infinity".to_string()
                } else {
                    format!("{value:e}")
                }
            };
            if !n.range.is_full() {
                parts.push(format!(
                    "(Number.isNaN(v) || (v >= {} && v <= {}))",
                    bound(n.range.min),
                    bound(n.range.max)
                ));
            }
            if n.int {
                parts.push("(Number.isNaN(v) || !Number.isFinite(v) || Number.isInteger(v))".into());
            }
            if n.non_neg_zero {
                parts.push("!Object.is(v, -0)".into());
            }
            format!("({})", parts.join(" && "))
        }
        Type::Boolean => "typeof v === 'boolean'".into(),
        Type::String => "typeof v === 'string'".into(),
        Type::BigInt => "typeof v === 'bigint'".into(),
        Type::Symbol => "typeof v === 'symbol'".into(),
        Type::Null => "v === null".into(),
        Type::Undefined => "v === undefined".into(),
        Type::Array(element) => format!("(Array.isArray(v) && v.every((v) => {}))", predicate(element)),
        Type::TypedArray(kind) => format!("(v instanceof {})", kind.constructor_name()),
        Type::Tuple(items) => {
            let mut parts = vec![format!("Array.isArray(v) && v.length === {}", items.len())];
            for (index, item) in items.iter().enumerate() {
                parts.push(format!("((v) => {})(v[{index}])", predicate(item)));
            }
            format!("({})", parts.join(" && "))
        }
        Type::Object(shape) => {
            let mut parts = vec!["typeof v === 'object' && v !== null".to_string()];
            for (key, ty) in &shape.properties {
                parts.push(format!("((v) => {})(v[{key:?}])", predicate(ty)));
            }
            if shape.exact {
                let keys: Vec<String> = shape.properties.iter().map(|(key, _)| format!("{key:?}")).collect();
                parts.push(format!(
                    "Object.getPrototypeOf(v) === Object.prototype && Object.getOwnPropertyNames(v).length === {} && [{}].every((k) => Object.hasOwn(v, k))",
                    keys.len(),
                    keys.join(", ")
                ));
            }
            format!("({})", parts.join(" && "))
        }
        Type::Instance(_) => "(typeof v === 'object' && v !== null)".into(),
        Type::Function(..) => "typeof v === 'function'".into(),
        Type::Union(members) => format!("({})", members.iter().map(predicate).collect::<Vec<_>>().join(" || ")),
    }
}

fn checked_kind(kind: AstKind<'_>) -> bool {
    matches!(
        kind,
        AstKind::IdentifierReference(_)
            | AstKind::StaticMemberExpression(_)
            | AstKind::ComputedMemberExpression(_)
            | AstKind::PrivateFieldExpression(_)
            | AstKind::CallExpression(_)
            | AstKind::NewExpression(_)
            | AstKind::BinaryExpression(_)
            | AstKind::LogicalExpression(_)
            | AstKind::UnaryExpression(_)
            | AstKind::UpdateExpression(_)
            | AstKind::AssignmentExpression(_)
            | AstKind::ConditionalExpression(_)
            | AstKind::ThisExpression(_)
            | AstKind::ArrayExpression(_)
            | AstKind::ObjectExpression(_)
            | AstKind::TSAsExpression(_)
            | AstKind::TSNonNullExpression(_)
            | AstKind::AwaitExpression(_)
    )
}

/// Whether wrapping the node in a call would change what the program does.
fn must_not_wrap(nodes: &AstNodes<'_>, id: NodeId) -> bool {
    let span = nodes.kind(id).span();
    let mut child = span;
    for ancestor in nodes.ancestor_ids(id) {
        let kind = nodes.kind(ancestor);
        match kind {
            AstKind::ChainExpression(_)
            | AstKind::TSEnumDeclaration(_)
            | AstKind::Decorator(_)
            | AstKind::ExportSpecifier(_)
            | AstKind::JSXElement(_)
            | AstKind::JSXFragment(_)
            | AstKind::JSXOpeningElement(_)
            | AstKind::JSXClosingElement(_)
            | AstKind::JSXMemberExpression(_)
            | AstKind::AssignmentTargetPropertyIdentifier(_)
            | AstKind::ArrayAssignmentTarget(_)
            | AstKind::ObjectAssignmentTarget(_)
            | AstKind::TSTypeAnnotation(_) => return true,
            AstKind::ParenthesizedExpression(_) | AstKind::TSAsExpression(_) | AstKind::TSNonNullExpression(_) | AstKind::TSSatisfiesExpression(_) | AstKind::TSTypeAssertion(_) => {
                child = kind.span();
                continue;
            }
            AstKind::CallExpression(call) if call.callee.span() == child => return true,
            AstKind::NewExpression(new) if new.callee.span() == child => return true,
            AstKind::TaggedTemplateExpression(tagged) if tagged.tag.span() == child => return true,
            AstKind::AssignmentExpression(assign) if assign.left.span() == child => return true,
            AstKind::UpdateExpression(update) if update.argument.span() == child => return true,
            AstKind::UnaryExpression(unary) if unary.operator.is_delete() || (unary.operator.is_typeof() && matches!(nodes.kind(id), AstKind::IdentifierReference(_))) => return true,
            AstKind::ForInStatement(statement) if statement.left.span() == child => return true,
            AstKind::ForOfStatement(statement) if statement.left.span() == child => return true,
            AstKind::ObjectProperty(property) if property.shorthand => return true,
            AstKind::AssignmentTargetWithDefault(_) | AstKind::AssignmentTargetRest(_) | AstKind::AssignmentTargetPropertyProperty(_) => return true,
            _ => return false,
        }
    }
    false
}

/// The module's source with every checkable proven fact wrapped in `__c(id, …)`, and the
/// predicates registered at the top.
fn instrument(facts: &Facts, module: ModuleId, source: &str, source_type: oxc_span::SourceType) -> (String, usize) {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    let program = allocator.alloc(parsed.program);
    let semantic = SemanticBuilder::new().with_enum_eval(true).with_build_nodes(true).build(program).semantic;
    let nodes = semantic.nodes();
    let mut predicates = String::new();
    let mut edits: Vec<(u32, i64, String)> = Vec::new();
    let mut count = 0;
    for node in nodes.iter() {
        if !checked_kind(node.kind()) {
            continue;
        }
        let fact = facts.ty(NodeRef { module, node: node.id() });
        if fact.trust != Trust::Proven || *fact.value == Type::Unknown {
            continue;
        }
        if must_not_wrap(nodes, node.id()) {
            continue;
        }
        // `super()` must stay a statement for TypeScript's parameter properties.
        if let AstKind::CallExpression(call) = node.kind()
            && matches!(call.callee, oxc_ast::ast::Expression::Super(_))
        {
            continue;
        }
        let span: Span = node.kind().span();
        let id = format!("m{}:{}", module.index(), node.id().index());
        writeln!(predicates, "  {id:?}: [(v) => {}, {:?}],", predicate(fact.value), format!("{:?} at {}..{}", fact.value, span.start, span.end)).unwrap();
        // Openings outer-first, closings inner-first.
        edits.push((span.start, i64::from(span.size()), format!("__c({id:?}, ")));
        edits.push((span.end, -i64::from(span.size()), ")".into()));
        count += 1;
    }
    edits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| {
        // At one offset: closings (negative) before openings, then by nesting.
        match (a.1 < 0, b.1 < 0) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => b.1.cmp(&a.1),
            (false, false) => b.1.cmp(&a.1),
        }
    }));
    let mut out = String::new();
    writeln!(out, "Object.assign(globalThis.__P, {{\n{predicates}}});").unwrap();
    let mut last = 0usize;
    for (at, _, text) in edits {
        out.push_str(&source[last..at as usize]);
        out.push_str(&text);
        last = at as usize;
    }
    out.push_str(&source[last..]);
    (out, count)
}

const RUNNER: &str = r#"
globalThis.__P = {};
const violations = [];
let checks = 0;
globalThis.__c = (id, value) => {
  checks++;
  const entry = globalThis.__P[id];
  if (entry && !entry[0](value)) {
    let shown;
    try { shown = typeof value === 'object' ? JSON.stringify(value) : String(value); } catch { shown = typeof value; }
    if (Object.is(value, -0)) shown = '-0';
    violations.push(`${id}: ${entry[1]} but got ${shown}`);
  }
  return value;
};
import { pathToFileURL } from 'node:url';
const m = await import(pathToFileURL(process.argv[2]).href);
const log = console.log;
console.log = () => {};
try { await m.main(); } finally { console.log = log; }
process.stdout.write('\n__CHECKS__' + JSON.stringify({ checks, violations }) + '\n');
"#;

#[test]
fn proven_facts_hold_at_run_time() {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("skipping: node is not on PATH");
        return;
    }
    let mut failures = Vec::new();
    let mut total = 0;
    let out_root = tempfile::tempdir().unwrap();
    for entry in common::all_fixtures() {
        let source = std::fs::read_to_string(&entry).unwrap();
        if !source.contains("export function main") && !source.contains("export async function main") {
            continue;
        }
        let dir = entry.parent().unwrap();
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let loaded = common::load(&entry);
        let options = common::options_for(dir);
        let out = out_root.path().join(&name);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("package.json"), "{ \"type\": \"module\" }\n").unwrap();
        for module in loaded.graph.modules() {
            let (instrumented, count) = instrument(&loaded.analysis.facts, module.id, &module.source, module.source_type);
            total += count;
            let stripped = trioxide::strip_types(dir, &module.path, &instrumented, &options).unwrap();
            assert!(
                !stripped.has_errors(),
                "{name}: instrumented {} doesn't compile: {:?}\n{instrumented}",
                module.path.display(),
                stripped.diagnostics
            );
            let relative = module.path.strip_prefix(dir).unwrap_or(Path::new(module.path.file_name().unwrap()));
            let target = out.join(relative).with_extension("js");
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, stripped.code).unwrap();
        }
        let runner = out.join("__runner.mjs");
        std::fs::write(&runner, RUNNER).unwrap();
        let entry_js = out.join(entry.strip_prefix(dir).unwrap()).with_extension("js");
        // The fixture's own helpers (e.g. the JSX runtime) resolve from the differential
        // package's node_modules.
        let modules = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/differential/node_modules");
        if modules.is_dir() && !out.join("node_modules").exists() {
            #[cfg(unix)]
            let _ = std::os::unix::fs::symlink(&modules, out.join("node_modules"));
        }
        let result = Command::new("node").arg(&runner).arg(&entry_js).output().unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        let Some(report) = stdout.lines().find_map(|line| line.strip_prefix("__CHECKS__")) else {
            failures.push(format!("{name}: did not run: {}", String::from_utf8_lossy(&result.stderr)));
            continue;
        };
        let report: serde_json::Value = serde_json::from_str(report).unwrap();
        eprintln!("{name}: {} runtime checks", report["checks"]);
        for violation in report["violations"].as_array().unwrap() {
            failures.push(format!("{name}: {}", violation.as_str().unwrap()));
        }
    }
    assert!(total > 0, "no proven facts were instrumented");
    assert!(failures.is_empty(), "proven facts contradicted at run time:\n{}", failures.join("\n"));
}
