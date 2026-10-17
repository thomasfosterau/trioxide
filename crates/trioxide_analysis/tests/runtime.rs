//! Checks `Proven` facts against reality: every analysis fixture with a `main()` is
//! instrumented, then run in Node, and any contradiction fails the test. Checked:
//!
//! - node types: every wrappable expression (and every parameter and declared binding, after
//!   it is bound) with a proven type, plus the proven type of the symbol an identifier reads;
//!   instances with `instanceof` against every evaluation of their class;
//! - in-bounds element accesses: the index is below the length and not a hole;
//! - effects: functions proven not to throw are run inside a `try` that reports a throw;
//! - stable shapes: objects from sites with a proven stable shape keep their own keys, in
//!   order, from creation to the end of the run.
//!
//! Not checked: escape levels and call edges (no runtime counterpart that is cheap to observe),
//! nodes inside `?.` chains, assignment targets and member callees.
//!
//! Skipped (with a message) when `node` is not on PATH; a fixture that imports a package is
//! skipped when the differential suite's dependencies aren't installed.

#![expect(clippy::print_stderr, reason = "the test reports what it skips")]

mod common;

use std::{fmt::Write as _, path::Path, process::Command};

use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_parser::Parser;
use oxc_semantic::{AstNodes, SemanticBuilder};
use oxc_span::{GetSpan, Span};
use trioxide_analysis::{AllocKind, Facts, Trust};
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
                parts
                    .push("(Number.isNaN(v) || !Number.isFinite(v) || Number.isInteger(v))".into());
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
        Type::Array(element) => format!(
            "(Array.isArray(v) && v.every((v) => {}))",
            predicate(element)
        ),
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
                let keys: Vec<String> = shape
                    .properties
                    .iter()
                    .map(|(key, _)| format!("{key:?}"))
                    .collect();
                parts.push(format!(
                    "Object.getPrototypeOf(v) === Object.prototype && Object.getOwnPropertyNames(v).length === {} && [{}].every((k) => Object.hasOwn(v, k))",
                    keys.len(),
                    keys.join(", ")
                ));
            }
            format!("({})", parts.join(" && "))
        }
        Type::Instance(class) => format!(
            "globalThis.__instance({:?}, v)",
            format!("m{}:{}", class.module.index(), class.node.index())
        ),
        Type::Function(..) => "typeof v === 'function'".into(),
        Type::Union(members) => format!(
            "({})",
            members
                .iter()
                .map(predicate)
                .collect::<Vec<_>>()
                .join(" || ")
        ),
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
            | AstKind::TemplateLiteral(_)
            | AstKind::SequenceExpression(_)
            | AstKind::ArrowFunctionExpression(_)
            | AstKind::YieldExpression(_)
            | AstKind::ParenthesizedExpression(_)
    ) || matches!(kind, AstKind::Function(function) if function.is_expression())
        || matches!(kind, AstKind::Class(class) if class.is_expression())
}

/// Whether wrapping the node in a call would change what the program does.
fn must_not_wrap(nodes: &AstNodes<'_>, id: NodeId) -> bool {
    let span = nodes.kind(id).span();
    // A method's function is part of its definition.
    if matches!(nodes.kind(id), AstKind::Function(_))
        && match nodes.parent_kind(id) {
            AstKind::ObjectProperty(property) => {
                property.method || property.kind != oxc_ast::ast::PropertyKind::Init
            }
            AstKind::MethodDefinition(_) => true,
            _ => false,
        }
    {
        return true;
    }
    let mut child = span;
    for ancestor in nodes.ancestor_ids(id) {
        let kind = nodes.kind(ancestor);
        #[expect(
            clippy::match_same_arms,
            reason = "distinct cases, documented separately"
        )]
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
            AstKind::ParenthesizedExpression(_)
            | AstKind::TSAsExpression(_)
            | AstKind::TSNonNullExpression(_)
            | AstKind::TSSatisfiesExpression(_)
            | AstKind::TSTypeAssertion(_) => {
                child = kind.span();
            }
            // A member callee would lose its `this`; a plain one keeps `undefined`.
            AstKind::CallExpression(call) if call.callee.span() == child => {
                return !matches!(nodes.kind(id), AstKind::IdentifierReference(_)) || span != child;
            }
            AstKind::NewExpression(new) if new.callee.span() == child => return true,
            AstKind::TaggedTemplateExpression(tagged) if tagged.tag.span() == child => return true,
            AstKind::AssignmentExpression(assign) if assign.left.span() == child => return true,
            AstKind::UpdateExpression(update) if update.argument.span() == child => return true,
            AstKind::UnaryExpression(unary)
                if unary.operator.is_delete()
                    || (unary.operator.is_typeof()
                        && matches!(nodes.kind(id), AstKind::IdentifierReference(_))) =>
            {
                return true;
            }
            AstKind::ForInStatement(statement) if statement.left.span() == child => return true,
            AstKind::ForOfStatement(statement) if statement.left.span() == child => return true,
            AstKind::ObjectProperty(property) if property.shorthand => return true,
            AstKind::AssignmentTargetWithDefault(_)
            | AstKind::AssignmentTargetRest(_)
            | AstKind::AssignmentTargetPropertyProperty(_) => return true,
            _ => return false,
        }
    }
    false
}

/// Insertions into a source: wrappers (an opening and a closing around a span) and plain
/// insertions, applied so that wrappers nest.
#[derive(Default)]
struct Edits {
    /// `(offset, size, sequence, text)`: openings have a positive size, closings a negative one.
    edits: Vec<(u32, i64, usize, String)>,
}

impl Edits {
    fn wrap(&mut self, span: Span, open: String, close: &str) {
        let sequence = self.edits.len();
        self.edits
            .push((span.start, i64::from(span.size()), sequence, open));
        self.edits
            .push((span.end, -i64::from(span.size()), sequence, close.into()));
    }

    /// Text at `offset` outside every wrapper that opens there (`first`) or inside every one
    /// that closes there.
    fn insert(&mut self, offset: u32, text: String, first: bool) {
        let sequence = self.edits.len();
        let size = if first { i64::MAX } else { i64::MIN };
        self.edits.push((offset, size, sequence, text));
    }

    fn apply(mut self, source: &str) -> String {
        self.edits.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| match (a.1 < 0, b.1 < 0) {
                // Closings first, innermost (smallest) first, the later-opened first.
                (true, true) => b.1.cmp(&a.1).then(b.2.cmp(&a.2)),
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                // Openings outermost first, the earlier first.
                (false, false) => b.1.cmp(&a.1).then(a.2.cmp(&b.2)),
            })
        });
        let mut out = String::new();
        let mut last = 0usize;
        for (at, _, _, text) in self.edits {
            out.push_str(&source[last..at as usize]);
            out.push_str(&text);
            last = at as usize;
        }
        out.push_str(&source[last..]);
        out
    }
}

/// The module's source instrumented with every check its facts allow, and how many.
#[expect(clippy::too_many_lines, reason = "one pass over the nodes")]
fn instrument(
    facts: &Facts,
    module: ModuleId,
    source: &str,
    source_type: oxc_span::SourceType,
) -> (String, usize) {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    let program = allocator.alloc(parsed.program);
    let semantic = SemanticBuilder::new()
        .with_enum_eval(true)
        .with_build_nodes(true)
        .build(program)
        .semantic;
    let nodes = semantic.nodes();
    let scoping = semantic.scoping();
    let mut predicates = String::new();
    let mut edits = Edits::default();
    let mut count = 0;
    let key = |node: NodeId| format!("m{}:{}", module.index(), node.index());
    let register = |predicates: &mut String, id: &str, ty: &Type, span: Span| {
        writeln!(
            predicates,
            "  {id:?}: [(v) => {}, {:?}],",
            predicate(ty),
            format!("{ty:?} at {}..{}", span.start, span.end)
        )
        .unwrap();
    };
    for node in nodes.iter() {
        let node_ref = NodeRef {
            module,
            node: node.id(),
        };
        let span: Span = node.kind().span();
        // Classes register themselves, so instance checks can use `instanceof`.
        if let AstKind::Class(class) = node.kind()
            && let Some(name) = &class.id
            && class.is_declaration()
        {
            let statement = match nodes.parent_kind(node.id()) {
                AstKind::ExportNamedDeclaration(export) => export.span,
                AstKind::ExportDefaultDeclaration(export) => export.span,
                _ => class.span,
            };
            edits.insert(
                statement.end,
                format!(";globalThis.__class({:?}, {});", key(node.id()), name.name),
                false,
            );
        }
        // Functions proven not to throw report it if they do.
        if let Some(body) = function_body(nodes, node.id()) {
            let effects = facts.effects(node_ref);
            if effects.trust == Trust::Proven && !effects.value.throws {
                let id = key(node.id());
                edits.insert(body.start + 1, "try {".into(), false);
                edits.insert(
                    body.end - 1,
                    format!("}} catch (e) {{ globalThis.__threw({id:?}); throw e; }}"),
                    true,
                );
                count += 1;
            }
        }
        // Parameters and declared bindings, once bound.
        if let AstKind::BindingIdentifier(binding) = node.kind() {
            let fact = facts.ty(node_ref);
            if fact.trust == Trust::Proven
                && *fact.value != Type::Unknown
                && let Some(at) = bound_at(nodes, node.id())
            {
                let id = key(node.id());
                register(&mut predicates, &id, fact.value, span);
                edits.insert(at, format!(";__c({id:?}, {});", binding.name), false);
                count += 1;
            }
            continue;
        }
        // In-bounds element accesses.
        if let AstKind::ComputedMemberExpression(member) = node.kind()
            && let fact = facts.in_bounds(node_ref)
            && fact.trust == Trust::Proven
            && fact.value
        {
            let id = key(node.id());
            edits.wrap(member.object.span(), "__bo(".into(), ")");
            edits.wrap(member.expression.span(), format!("__bi({id:?}, "), ")");
            count += 1;
        }
        if !checked_kind(node.kind()) || must_not_wrap(nodes, node.id()) {
            continue;
        }
        // `super()` must stay a statement for TypeScript's parameter properties.
        if let AstKind::CallExpression(call) = node.kind()
            && matches!(call.callee, oxc_ast::ast::Expression::Super(_))
        {
            continue;
        }
        let fact = facts.ty(node_ref);
        if fact.trust == Trust::Proven && *fact.value != Type::Unknown {
            let id = key(node.id());
            register(&mut predicates, &id, fact.value, span);
            edits.wrap(span, format!("__c({id:?}, ("), "))");
            count += 1;
        }
        // The symbol an identifier reads has a type of its own.
        if let AstKind::IdentifierReference(ident) = node.kind()
            && let Some(symbol) = ident
                .reference_id
                .get()
                .map(|reference| scoping.get_reference(reference))
                .filter(|reference| reference.is_value() && !reference.is_type())
                .and_then(oxc_semantic::Reference::symbol_id)
            && let fact = facts.symbol_ty(trioxide_common::SymbolRef { module, symbol })
            && fact.trust == Trust::Proven
            && *fact.value != Type::Unknown
        {
            let id = format!("{}s", key(node.id()));
            register(&mut predicates, &id, fact.value, span);
            edits.wrap(span, format!("__c({id:?}, ("), "))");
            count += 1;
        }
        // Objects whose shape is proven stable.
        if matches!(
            node.kind(),
            AstKind::ObjectExpression(_) | AstKind::NewExpression(_)
        ) && let Some(allocation) = facts.allocation(node_ref)
            && matches!(allocation.kind, AllocKind::Object | AllocKind::Instance(_))
            && allocation.stable_shape.trust == Trust::Proven
            && allocation.stable_shape.value
        {
            edits.wrap(span, format!("__s({:?}, ", key(node.id())), ")");
            count += 1;
        }
    }
    let mut out = String::new();
    writeln!(out, "Object.assign(globalThis.__P, {{\n{predicates}}});").unwrap();
    out.push_str(&edits.apply(source));
    (out, count)
}

/// The block body of a function that may be wrapped in a `try` (not a constructor, whose
/// `super()` and parameter properties must come first).
fn function_body(nodes: &AstNodes<'_>, id: NodeId) -> Option<Span> {
    let kind = nodes.kind(id);
    if matches!(nodes.parent_kind(id), AstKind::MethodDefinition(method) if method.kind.is_constructor())
    {
        return None;
    }
    match kind {
        AstKind::Function(function) => {
            let body = function.body.as_ref()?;
            Some(body.span)
        }
        AstKind::ArrowFunctionExpression(arrow) => block_body(&arrow.body),
        _ => None,
    }
    .filter(|_| !matches!(kind, AstKind::Function(function) if function.r#type == oxc_ast::ast::FunctionType::TSDeclareFunction))
}

/// An arrow's body, if it is a block.
fn block_body(body: &oxc_ast::ast::ArrowFunctionBody<'_>) -> Option<Span> {
    match body {
        oxc_ast::ast::ArrowFunctionBody::FunctionBody(body) => Some(body.span),
        _ => None,
    }
}

/// Where a check of the binding `id` can go once it holds its value: the start of its
/// function's body for a parameter (not a constructor's), after the statement for a
/// declaration in a statement list.
fn bound_at(nodes: &AstNodes<'_>, id: NodeId) -> Option<u32> {
    let mut previous = id;
    for ancestor in nodes.ancestor_ids(id) {
        match nodes.kind(ancestor) {
            AstKind::FormalParameters(_) => {
                let function = nodes.parent_id(ancestor);
                if matches!(nodes.parent_kind(function), AstKind::MethodDefinition(method) if method.kind.is_constructor())
                {
                    return None;
                }
                return match nodes.kind(function) {
                    AstKind::Function(function) => {
                        function.body.as_ref().map(|body| body.span.start + 1)
                    }
                    AstKind::ArrowFunctionExpression(arrow) => {
                        block_body(&arrow.body).map(|body| body.start + 1)
                    }
                    _ => None,
                };
            }
            AstKind::VariableDeclaration(declaration) => {
                let parent = nodes.parent_kind(ancestor);
                let statement = match parent {
                    AstKind::Program(_)
                    | AstKind::BlockStatement(_)
                    | AstKind::FunctionBody(_)
                    | AstKind::StaticBlock(_)
                    | AstKind::SwitchCase(_) => declaration.span,
                    AstKind::ExportNamedDeclaration(export) => export.span,
                    _ => return None,
                };
                if declaration.declare {
                    return None;
                }
                let _ = previous;
                return Some(statement.end);
            }
            AstKind::Function(_)
            | AstKind::ArrowFunctionExpression(_)
            | AstKind::Class(_)
            | AstKind::CatchClause(_)
            | AstKind::ForStatement(_)
            | AstKind::ForInStatement(_)
            | AstKind::ForOfStatement(_) => return None,
            _ => {}
        }
        previous = ancestor;
    }
    None
}

const RUNNER: &str = r"
globalThis.__P = {};
const violations = [];
let checks = 0;
const classes = new Map();
globalThis.__class = (id, c) => {
  if (!classes.has(id)) classes.set(id, new Set());
  classes.get(id).add(c);
};
globalThis.__instance = (id, v) => {
  if (typeof v !== 'object' || v === null) return false;
  const all = classes.get(id);
  return !all || [...all].some((c) => v instanceof c);
};
const objects = [];
globalThis.__s = (id, v) => {
  checks++;
  objects.push([id, v, Object.getOwnPropertyNames(v).join(',')]);
  return v;
};
const arrays = [];
globalThis.__bo = (a) => { arrays.push(a); return a; };
globalThis.__bi = (id, i) => {
  checks++;
  const a = arrays.pop();
  if (!(typeof i === 'number' && Number.isInteger(i) && i >= 0 && i < a.length && i in a)) {
    violations.push(`${id}: in bounds but index ${String(i)} of length ${a.length}`);
  }
  return i;
};
globalThis.__threw = (id) => { violations.push(`${id}: proven not to throw, but threw`); };
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
for (const [id, v, keys] of objects) {
  const now = Object.getOwnPropertyNames(v).join(',');
  if (now !== keys) violations.push(`${id}: stable shape [${keys}] but ended as [${now}]`);
}
process.stdout.write('\n__CHECKS__' + JSON.stringify({ checks, violations }) + '\n');
";

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
        if !source.contains("export function main")
            && !source.contains("export async function main")
        {
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
            let (instrumented, count) = instrument(
                &loaded.analysis.facts,
                module.id,
                &module.source,
                module.source_type,
            );
            total += count;
            let stripped =
                trioxide::strip_types(dir, &module.path, &instrumented, &options).unwrap();
            assert!(
                !stripped.has_errors(),
                "{name}: instrumented {} doesn't compile: {:?}\n{instrumented}",
                module.path.display(),
                stripped.diagnostics
            );
            let relative = module
                .path
                .strip_prefix(dir)
                .unwrap_or(Path::new(module.path.file_name().unwrap()));
            let target = out.join(relative).with_extension("js");
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, stripped.code).unwrap();
        }
        let runner = out.join("__runner.mjs");
        std::fs::write(&runner, RUNNER).unwrap();
        let entry_js = out
            .join(entry.strip_prefix(dir).unwrap())
            .with_extension("js");
        // The fixture's own helpers (e.g. the JSX runtime) resolve from the differential
        // package's node_modules.
        let modules =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/differential/node_modules");
        let imports_package = loaded.graph.modules().iter().any(|module| {
            module
                .imports
                .iter()
                .any(|import| !import.specifier.starts_with('.'))
        });
        if imports_package && !modules.is_dir() {
            eprintln!("{name}: skipped (run `pnpm install` for its packages)");
            continue;
        }
        if modules.is_dir() && !out.join("node_modules").exists() {
            #[cfg(unix)]
            let _ = std::os::unix::fs::symlink(&modules, out.join("node_modules"));
        }
        let result = Command::new("node")
            .arg(&runner)
            .arg(&entry_js)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        let Some(report) = stdout
            .lines()
            .find_map(|line| line.strip_prefix("__CHECKS__"))
        else {
            failures.push(format!(
                "{name}: did not run: {}",
                String::from_utf8_lossy(&result.stderr)
            ));
            continue;
        };
        let report: serde_json::Value = serde_json::from_str(report).unwrap();
        eprintln!("{name}: {} runtime checks", report["checks"]);
        for violation in report["violations"].as_array().unwrap() {
            failures.push(format!("{name}: {}", violation.as_str().unwrap()));
        }
    }
    assert!(total > 0, "no proven facts were instrumented");
    assert!(
        failures.is_empty(),
        "proven facts contradicted at run time:\n{}",
        failures.join("\n")
    );
}
