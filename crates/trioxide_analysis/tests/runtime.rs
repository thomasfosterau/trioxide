//! Checks `Proven` facts against reality: every analysis fixture with a `main()` is
//! instrumented, then run in Node, and any contradiction fails the test. Checked:
//!
//! - types: every expression that can be wrapped without changing the program (including
//!   template literals, sequences, function and class expressions, `yield`, `super.x`,
//!   whole `?.` chains and plain callees), every parameter (at body start) and declared
//!   binding (after its statement), and the symbol type behind every value reference;
//!   instances with `instanceof` against every evaluation of their class, and functions by
//!   identity against every evaluation of their definition;
//! - in-bounds accesses: the index is an integer below the length, and not a hole;
//! - unit effects: a function, arrow (expression-bodied ones included), constructor (unless
//!   it has parameter properties) or static block proven not to throw reports any throw; one
//!   proven not to write (and not async or a generator) reports any write, while it runs, to
//!   a top-level binding or an object reached from one; instance field initialisers are
//!   checked against their class's init unit;
//! - expression effects: every expression proven not to write reports a write while it
//!   evaluates to a binding of its own function's activation, to an object reached from one,
//!   or to module-level state; one proven not to throw reports a throw (expressions around an
//!   `await` or `yield` are skipped);
//! - call sites proven complete: the first unit to run after the arguments are evaluated (or
//!   a builtin it calls) is one of the site's callees;
//! - loop invariants: each invariant expression has the same value (`Object.is`) on every
//!   iteration of one entry into its loop;
//! - shapes: objects from allocation sites with a proven stable shape, and instances of
//!   classes with a proven stable shape, end the run with the own keys they had when made,
//!   in order, and a class's proven fields come first in that order;
//! - reachability: units and statements proven unreachable report it if they run.
//!
//! Not checked: escape levels, `new` sites, sites with spread arguments or a member callee
//! and no arguments, points-to sets, the other loop facts, writes through `this` or a call
//! result outside the writing unit, writes by builtins (`push`), the parts of a `?.` chain,
//! member callees and assignment targets themselves.
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
use rustc_hash::FxHashSet;
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
        Type::Function(_, Some(function)) => format!(
            "globalThis.__function({:?}, v)",
            format!("m{}:{}", function.module.index(), function.node.index())
        ),
        Type::Function(_, None) => "typeof v === 'function'".into(),
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
            | AstKind::ChainExpression(_)
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
    let mut activations: FxHashSet<NodeId> = FxHashSet::default();
    // `await` and `yield`, with their unit: expressions around them may suspend.
    let suspensions: Vec<(Span, NodeId)> = nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind(),
                AstKind::AwaitExpression(_) | AstKind::YieldExpression(_)
            )
        })
        .map(|node| (node.kind().span(), unit_of(nodes, node.id(), false)))
        .collect();
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
        // Code proven unreachable reports it if it runs.
        let mut dead_unit = false;
        if facts.reachable(node_ref).trust == Trust::Proven && !facts.reachable(node_ref).value {
            let id = key(node.id());
            let report = format!("globalThis.__dead({id:?});");
            if unit_body(nodes, node.id(), source).is_some() {
                dead_unit = true;
                count += 1;
            } else if is_wrappable_statement(node.kind())
                && !facts
                    .reachable(NodeRef {
                        module,
                        node: nodes.parent_id(node.id()),
                    })
                    .trust
                    .eq(&Trust::Proven)
            {
                edits.insert(span.start, format!("{{{report}"), true);
                edits.insert(span.end, "}".into(), false);
                count += 1;
            }
        }
        // Units report their activation (for site checks and local writes), a throw when
        // proven not to throw, and a write to module-level state while they run when proven
        // not to write (outside their locals).
        if let Some(body) = unit_body(nodes, node.id(), source) {
            let effects = facts.effects(node_ref);
            let id = key(node.id());
            let no_throw = effects.trust == Trust::Proven && !effects.value.throws;
            let suspends = matches!(node.kind(), AstKind::Function(function) if function.r#async || function.generator)
                || matches!(node.kind(), AstKind::ArrowFunctionExpression(arrow) if arrow.r#async);
            let no_write = effects.trust == Trust::Proven && !effects.value.writes && !suspends;
            let mut open = String::new();
            let mut close = String::new();
            if dead_unit {
                write!(open, "globalThis.__dead({id:?});").unwrap();
            }
            write!(
                open,
                "const __A{} = globalThis.__ran({id:?});",
                node.id().index()
            )
            .unwrap();
            activations.insert(node.id());
            if no_throw {
                open.push_str("try {");
                count += 1;
            }
            if no_write {
                write!(open, "try {{ globalThis.__quiet.push({id:?});").unwrap();
                close.push_str("} finally { globalThis.__quiet.pop(); }");
                count += 1;
            }
            if no_throw {
                write!(
                    close,
                    "}} catch (e) {{ globalThis.__threw({id:?}); throw e; }}"
                )
                .unwrap();
            }
            match body {
                UnitBody::Block(body) => {
                    edits.insert(body.start + 1, open, true);
                    edits.insert(body.end - 1, close, true);
                }
                UnitBody::Expression(body) => {
                    edits.wrap(body, format!("{{{open}return ("), &format!(");{close}}}"));
                }
            }
        }
        // Instance field initialisers, against their class's init unit.
        if let AstKind::PropertyDefinition(property) = node.kind()
            && !property.r#static
            && let Some(value) = &property.value
        {
            let init = NodeRef {
                module,
                node: nodes.parent_id(node.id()),
            };
            let effects = facts.effects(init);
            if effects.trust == Trust::Proven && (!effects.value.writes || !effects.value.throws) {
                edits.wrap(
                    value.span(),
                    format!(
                        "globalThis.__x(globalThis.__xpush({:?}, {}, {}, null), (",
                        key(init.node),
                        !effects.value.writes,
                        !effects.value.throws
                    ),
                    "))",
                );
                count += 1;
            }
        }
        // Writes, with the activation whose state they change (`'M'`: module-level state).
        if let Some((target, root)) = written_target(node.kind(), scoping)
            && !must_not_wrap(nodes, node.id())
        {
            let act = match root {
                WriteRoot::Module => "'M'".to_string(),
                WriteRoot::Scope(scope) => {
                    let declared = unit_of(nodes, scoping.get_node_id(scope), true);
                    activation(nodes, node.id(), declared, &activations)
                }
                WriteRoot::Here => activation(
                    nodes,
                    node.id(),
                    unit_of(nodes, node.id(), false),
                    &activations,
                ),
            };
            edits.wrap(
                span,
                format!("(globalThis.__wrote({target:?}, {act}), "),
                ")",
            );
        }
        // Calls at sites proven complete check which unit runs.
        if let AstKind::CallExpression(call) = node.kind()
            && let resolution = facts.site_resolution(node_ref)
            && resolution.complete.trust == Trust::Proven
            && resolution.complete.value
            && !matches!(call.callee, oxc_ast::ast::Expression::Super(_))
            && !must_not_wrap(nodes, node.id())
            && !call.arguments.iter().any(oxc_ast::ast::Argument::is_spread)
        {
            let allowed: Vec<String> = resolution
                .callees
                .iter()
                .map(|callee| {
                    format!(
                        "{:?}",
                        format!("m{}:{}", callee.module.index(), callee.node.index())
                    )
                })
                .collect();
            let allowed = format!("[{}]", allowed.join(", "));
            let id = key(node.id());
            let armed = if let Some(last) = call.arguments.last() {
                edits.wrap(
                    last.span(),
                    format!("globalThis.__last({id:?}, {allowed}, ("),
                    "))",
                );
                true
            } else if let oxc_ast::ast::Expression::Identifier(_) = &call.callee {
                edits.wrap(
                    call.callee.span(),
                    format!("(globalThis.__site({id:?}, {allowed}), "),
                    ")",
                );
                true
            } else {
                false
            };
            if armed {
                edits.wrap(span, format!("globalThis.__done({id:?}, ("), "))");
                count += 1;
            }
        }
        // Loop-invariant expressions keep their value across a loop entry's iterations.
        if let Some(loop_facts) = facts.loop_facts(node_ref)
            && !loop_facts.invariant.is_empty()
        {
            let mut statement = node.id();
            // A label stays on its loop.
            while matches!(nodes.parent_kind(statement), AstKind::LabeledStatement(_)) {
                statement = nodes.parent_id(statement);
            }
            let map = format!("__L{}", node.id().index());
            edits.wrap(
                nodes.kind(statement).span(),
                format!("{{const {map} = new Map();"),
                "}",
            );
            for expression in &loop_facts.invariant {
                if must_not_wrap(nodes, expression.node) {
                    continue;
                }
                edits.wrap(
                    nodes.kind(expression.node).span(),
                    format!("globalThis.__inv({map}, {:?}, (", key(expression.node)),
                    "))",
                );
                count += 1;
            }
        }
        // Function objects register themselves, for identity checks.
        match node.kind() {
            AstKind::Function(function) if function.is_declaration() && function.body.is_some() => {
                if let Some(name) = &function.id
                    && matches!(
                        nodes.parent_kind(node.id()),
                        AstKind::Program(_)
                            | AstKind::BlockStatement(_)
                            | AstKind::FunctionBody(_)
                            | AstKind::ExportNamedDeclaration(_)
                    )
                {
                    let statement = match nodes.parent_kind(node.id()) {
                        AstKind::ExportNamedDeclaration(export) => export.span,
                        _ => function.span,
                    };
                    edits.insert(
                        statement.end,
                        format!(";globalThis.__fn({:?}, {});", key(node.id()), name.name),
                        false,
                    );
                }
            }
            _ => {}
        }
        // Instances of classes with a proven stable shape keep their keys.
        if let AstKind::NewExpression(_) = node.kind()
            && let Some(allocation) = facts.allocation(node_ref)
            && let AllocKind::Instance(class) = allocation.kind
            && let shape = facts.class_shape(class)
            && shape.trust == Trust::Proven
            && shape.value.is_stable()
            && !must_not_wrap(nodes, node.id())
        {
            let fields: Vec<String> = shape
                .value
                .fields
                .iter()
                .map(|field| format!("{field:?}"))
                .collect();
            edits.wrap(
                span,
                format!(
                    "globalThis.__shape({:?}, [{}], (",
                    format!("m{}:{}", class.module.index(), class.node.index()),
                    fields.join(", ")
                ),
                "))",
            );
            count += 1;
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
        // Expressions proven not to write or not to throw.
        if effect_checked_kind(node.kind())
            && !suspensions.iter().any(|&(at, unit)| {
                span.start <= at.start
                    && at.end <= span.end
                    && unit == unit_of(nodes, node.id(), false)
            })
            && let effects = facts.expr_effects(node_ref)
            && effects.trust == Trust::Proven
            && (!effects.value.writes || !effects.value.throws)
        {
            let act = activation(
                nodes,
                node.id(),
                unit_of(nodes, node.id(), false),
                &activations,
            );
            edits.wrap(
                span,
                format!(
                    "globalThis.__x(globalThis.__xpush({:?}, {}, {}, {act}), (",
                    key(node.id()),
                    !effects.value.writes,
                    !effects.value.throws
                ),
                "))",
            );
            count += 1;
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
        // Function expressions register themselves (inside the check of their own type, so
        // they are registered when it runs).
        if (matches!(node.kind(), AstKind::Function(function) if function.is_expression())
            || matches!(node.kind(), AstKind::ArrowFunctionExpression(_)))
            && !must_not_wrap(nodes, node.id())
        {
            edits.wrap(
                span,
                format!("globalThis.__fne({:?}, (", key(node.id())),
                "))",
            );
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

/// The code of a unit that the harness can wrap.
enum UnitBody {
    /// A block, braces included.
    Block(Span),
    /// An arrow's expression body.
    Expression(Span),
}

/// The body of the unit `id` (a function, arrow or static block) that may be wrapped in a
/// `try`: not a constructor with parameter properties (they must follow `super()`), nor a
/// declaration without a body.
fn unit_body(nodes: &AstNodes<'_>, id: NodeId, source: &str) -> Option<UnitBody> {
    match nodes.kind(id) {
        AstKind::Function(function) => {
            if function.r#type == oxc_ast::ast::FunctionType::TSDeclareFunction {
                return None;
            }
            if matches!(nodes.parent_kind(id), AstKind::MethodDefinition(method) if method.kind.is_constructor())
                && function
                    .params
                    .items
                    .iter()
                    .any(oxc_ast::ast::FormalParameter::has_modifier)
            {
                return None;
            }
            function
                .body
                .as_ref()
                .map(|body| UnitBody::Block(body.span))
        }
        AstKind::ArrowFunctionExpression(arrow) => Some(match &arrow.body {
            oxc_ast::ast::ArrowFunctionBody::FunctionBody(body) => UnitBody::Block(body.span),
            body => UnitBody::Expression(body.as_expression()?.span()),
        }),
        AstKind::StaticBlock(block) => {
            let brace = source[block.span.start as usize..].find('{')?;
            Some(UnitBody::Block(Span::new(
                block.span.start + u32::try_from(brace).ok()?,
                block.span.end,
            )))
        }
        _ => None,
    }
}

/// The unit (function, arrow, static block or program) around `id`, or `id` itself when
/// `inclusive`.
fn unit_of(nodes: &AstNodes<'_>, id: NodeId, inclusive: bool) -> NodeId {
    let mut candidates = inclusive
        .then_some(id)
        .into_iter()
        .chain(nodes.ancestor_ids(id));
    candidates
        .find(|&candidate| {
            matches!(
                nodes.kind(candidate),
                AstKind::Function(_)
                    | AstKind::ArrowFunctionExpression(_)
                    | AstKind::StaticBlock(_)
                    | AstKind::PropertyDefinition(_)
                    | AstKind::AccessorProperty(_)
                    | AstKind::Program(_)
            )
        })
        .unwrap_or(id)
}

/// The JS expression for the activation of `unit` as seen from `at`: `'M'` for the module,
/// its `__A` binding when it has one and `at` is past its parameters, else `null`.
fn activation(
    nodes: &AstNodes<'_>,
    at: NodeId,
    unit: NodeId,
    activations: &FxHashSet<NodeId>,
) -> String {
    if matches!(nodes.kind(unit), AstKind::Program(_)) {
        return "'M'".into();
    }
    if !activations.contains(&unit) {
        return "null".into();
    }
    let in_params = nodes
        .ancestor_ids(at)
        .take_while(|&ancestor| ancestor != unit)
        .any(|ancestor| matches!(nodes.kind(ancestor), AstKind::FormalParameters(_)));
    if in_params {
        "null".into()
    } else {
        format!("__A{}", unit.index())
    }
}

/// Whose state a write changes.
enum WriteRoot {
    /// A top-level binding, or an object reached from one.
    Module,
    /// A binding declared in this scope, or an object reached from one.
    Scope(oxc_semantic::ScopeId),
    /// Something else (`this.x`, `f().x`, a pattern): taken as the writing unit's own.
    Here,
}

/// For an assignment, update or `delete`, what it writes and whose state that is.
fn written_target(
    kind: AstKind<'_>,
    scoping: &oxc_semantic::Scoping,
) -> Option<(String, WriteRoot)> {
    use oxc_ast::ast::{Expression, SimpleAssignmentTarget};
    let target = match kind {
        AstKind::AssignmentExpression(assign) => match assign.left.as_simple_assignment_target() {
            Some(target) => target,
            None => return Some(("a pattern".into(), WriteRoot::Here)),
        },
        AstKind::UpdateExpression(update) => &update.argument,
        AstKind::UnaryExpression(unary) if unary.operator.is_delete() => {
            let (Expression::StaticMemberExpression(_) | Expression::ComputedMemberExpression(_)) =
                unary.argument.without_parentheses()
            else {
                return None;
            };
            let mut object = unary.argument.without_parentheses();
            while let Some(member) = object.as_member_expression() {
                object = member.object().without_parentheses();
            }
            return Some(match object {
                Expression::Identifier(ident) => root(ident, scoping),
                _ => ("delete".into(), WriteRoot::Here),
            });
        }
        _ => return None,
    };
    let ident = match target {
        SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) => &**ident,
        target => {
            let mut object = target
                .as_member_expression()?
                .object()
                .without_parentheses();
            while let Some(member) = object.as_member_expression() {
                object = member.object().without_parentheses();
            }
            match object {
                Expression::Identifier(ident) => &**ident,
                _ => return Some(("a property".into(), WriteRoot::Here)),
            }
        }
    };
    Some(root(ident, scoping))
}

/// The binding `ident` refers to, as a write root.
fn root(
    ident: &oxc_ast::ast::IdentifierReference<'_>,
    scoping: &oxc_semantic::Scoping,
) -> (String, WriteRoot) {
    let name = ident.name.to_string();
    let Some(symbol) = ident
        .reference_id
        .get()
        .and_then(|reference| scoping.get_reference(reference).symbol_id())
    else {
        // A global.
        return (name, WriteRoot::Module);
    };
    let scope = scoping.symbol_scope_id(symbol);
    if scope == scoping.root_scope_id() {
        (name, WriteRoot::Module)
    } else {
        (name, WriteRoot::Scope(scope))
    }
}

/// Kinds whose effects are checked while they evaluate (not plain references, literals or
/// function and class values, whose evaluation does nothing).
fn effect_checked_kind(kind: AstKind<'_>) -> bool {
    matches!(
        kind,
        AstKind::StaticMemberExpression(_)
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
            | AstKind::ArrayExpression(_)
            | AstKind::ObjectExpression(_)
            | AstKind::TemplateLiteral(_)
            | AstKind::SequenceExpression(_)
            | AstKind::ChainExpression(_)
    )
}

/// A statement that can sit inside a block without changing what it declares.
fn is_wrappable_statement(kind: AstKind<'_>) -> bool {
    matches!(
        kind,
        AstKind::ExpressionStatement(_)
            | AstKind::ReturnStatement(_)
            | AstKind::ThrowStatement(_)
            | AstKind::IfStatement(_)
            | AstKind::ForStatement(_)
            | AstKind::WhileStatement(_)
            | AstKind::DoWhileStatement(_)
            | AstKind::BlockStatement(_)
    ) || matches!(kind, AstKind::VariableDeclaration(declaration) if declaration.kind == oxc_ast::ast::VariableDeclarationKind::Var)
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
const show = (value) => {
  try { return typeof value === 'object' && value !== null ? JSON.stringify(value) : Object.is(value, -0) ? '-0' : String(value); } catch { return typeof value; }
};
globalThis.__quiet = [];
// Expressions being evaluated: an entry left behind by a newer one threw.
const xs = [];
let tokens = 0;
globalThis.__xpush = (id, w, t, act) => {
  xs.push({ token: ++tokens, id, w, t, act: act ?? {}, wrote: null });
  return tokens;
};
globalThis.__x = (token, v) => {
  checks++;
  while (xs.length > 0) {
    const e = xs.pop();
    if (e.token === token) {
      if (e.wrote !== null) violations.push(`${e.id}: proven not to write, but wrote ${e.wrote}`);
      return v;
    }
    if (e.t) violations.push(`${e.id}: proven not to throw, but threw`);
  }
  return v;
};
globalThis.__wrote = (name, act) => {
  if (act === 'M' && globalThis.__quiet.length > 0) {
    violations.push(`${globalThis.__quiet.at(-1)}: proven not to write, but wrote ${name}`);
  }
  for (const e of xs) {
    if (e.w && e.wrote === null && (act === 'M' || e.act === act)) e.wrote = name;
  }
};
// Call sites proven complete: the first unit to run must be one of their callees.
const sites = [];
globalThis.__ran = (id) => {
  const top = sites.at(-1);
  if (top && !top.done) {
    if (top.allowed.includes(id)) top.done = true;
    else top.bad.push(id);
  }
  return {};
};
globalThis.__site = (key, allowed) => { sites.push({ key, allowed, done: false, bad: [] }); };
globalThis.__last = (key, allowed, v) => { globalThis.__site(key, allowed); return v; };
globalThis.__done = (key, v) => {
  checks++;
  while (sites.length > 0) {
    const s = sites.pop();
    if (s.key === key) {
      if (!s.done && s.bad.length > 0) violations.push(`${key}: complete with callees [${s.allowed}] but ran ${s.bad}`);
      return v;
    }
  }
  return v;
};
globalThis.__inv = (m, key, v) => {
  checks++;
  if (!m.has(key)) m.set(key, v);
  else if (!Object.is(m.get(key), v)) violations.push(`${key}: loop-invariant, but was ${show(m.get(key))} then ${show(v)}`);
  return v;
};
const functions = new Map();
const register = (id, f) => {
  if (!functions.has(id)) functions.set(id, new WeakSet());
  functions.get(id).add(f);
  return f;
};
globalThis.__fn = register;
globalThis.__fne = register;
globalThis.__function = (id, v) => {
  if (typeof v !== 'function') return false;
  const all = functions.get(id);
  return !all || all.has(v);
};
const instances = [];
globalThis.__shape = (id, fields, v) => {
  checks++;
  instances.push([id, fields, v, Object.getOwnPropertyNames(v).join(',')]);
  return v;
};
globalThis.__dead = (id) => { violations.push(`${id}: proven unreachable, but ran`); };
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
for (let [id, fields, v, keys] of instances) {
  const now = Object.getOwnPropertyNames(v);
  if (now.join(',') !== keys) violations.push(`${id}: stable class shape [${keys}] but ended as [${now}]`);
  // Private names aren't properties.
  fields = fields.filter((field) => !field.startsWith('#'));
  if (now.slice(0, fields.length).join(',') !== fields.join(',')) {
    violations.push(`${id}: fields [${fields}] first, but has [${now}]`);
  }
}
for (const e of xs) {
  if (e.t) violations.push(`${e.id}: proven not to throw, but threw`);
}
for (const [id, v, keys] of objects) {
  const now = Object.getOwnPropertyNames(v).join(',');
  if (now !== keys) violations.push(`${id}: stable shape [${keys}] but ended as [${now}]`);
}
process.stdout.write('\n__CHECKS__' + JSON.stringify({ checks, violations }) + '\n');
";

#[test]
fn proven_facts_hold_at_run_time() {
    check_at_run_time(0, 0);
}

/// One unit per chunk: every fact goes through the parallel interpreter's merging.
#[test]
fn proven_facts_hold_at_run_time_with_tiny_chunks() {
    check_at_run_time(1, 0);
}

/// Entries that still change after the first round are given up, in tiny chunks and in the
/// default ones: giving up keeps every fact sound however the units are chunked.
#[test]
fn proven_facts_hold_at_run_time_when_entries_are_given_up() {
    check_at_run_time(1, 1);
    check_at_run_time(0, 1);
}

fn check_at_run_time(chunk_units: usize, saturate_after: usize) {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("skipping: node is not on PATH");
        return;
    }
    let mut failures = Vec::new();
    let mut total = 0;
    let mut given_up = 0;
    let out_root = tempfile::tempdir().unwrap();
    // `FIXTURE=name` runs only the fixtures whose directory name contains `name`.
    let filter = std::env::var("FIXTURE").ok();
    for entry in common::all_fixtures() {
        let dir_name = entry.parent().and_then(|dir| dir.file_name());
        if filter.as_ref().is_some_and(|filter| {
            !dir_name.is_some_and(|name| name.to_string_lossy().contains(filter.as_str()))
        }) {
            continue;
        }
        let source = std::fs::read_to_string(&entry).unwrap();
        if !source.contains("export function main")
            && !source.contains("export async function main")
        {
            continue;
        }
        let dir = entry.parent().unwrap();
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let loaded = common::load_with(&entry, chunk_units, 0, saturate_after);
        given_up += loaded.analysis.stats.saturated;
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
    if saturate_after > 0 {
        assert!(given_up > 0, "no entry was given up");
    }
    assert!(
        failures.is_empty(),
        "proven facts contradicted at run time:\n{}",
        failures.join("\n")
    );
}
