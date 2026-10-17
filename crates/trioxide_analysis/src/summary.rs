//! Turns the interpreter's tables into [`Facts`].

use oxc_span::Span;
use oxc_syntax::symbol::SymbolId;
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::{NodeId, NodeRef, SymbolRef};
use trioxide_types::{Shape, Type};

use crate::{
    AnalysisOptions, callgraph,
    facts::{
        AllocKind, Allocation, ClassShape, Effects, Escape, Fact, Facts, FunctionInfo, LoopFacts,
        Trust,
    },
    interp::{self, ObjKind, Results, State},
    link::{Links, Target},
    value::{MAX_TYPE_DEPTH, Pts, Val},
    world::{Body, World},
};

/// Builds the fact tables.
pub fn facts(
    world: &World<'_>,
    links: &Links,
    results: &Results,
    options: &AnalysisOptions,
) -> Facts {
    let mut facts = Facts::new(options.trust_types);
    for (import, target) in links.imports() {
        if let Target::Symbol(target) = target {
            facts.set_import(import, target);
        }
    }
    for &unit in &world.unit_order {
        let info = world.unit(unit);
        facts.set_function(
            unit,
            FunctionInfo {
                name: info.name.clone(),
                span: info.span,
                open: interp::unit_open(world, &results.state, info),
            },
        );
    }
    if !results.converged {
        // Nothing the last round computed can be trusted.
        return facts;
    }
    let state = &results.state;
    for (&node, value) in &results.output.nodes {
        facts.set_ty(node, materialize(value, state));
    }
    for (&symbol, value) in &state.symbols {
        facts.set_symbol_ty(symbol, materialize(value, state));
    }
    for (&node, &trust) in &results.output.in_bounds {
        facts.set_in_bounds(node, trust);
    }
    let graph = callgraph::build(world, &results.output);
    let summaries = callgraph::effects(world, &graph, &results.output, state);
    for (&unit, &(proven, declared)) in &summaries {
        facts.set_effect_summaries(unit, proven, declared);
    }
    usage_facts(&mut facts, world, results, &graph, &summaries);
    facts.set_call_graph(graph);

    let mut instances: FxHashMap<NodeRef, Vec<&interp::Obj>> = FxHashMap::default();
    for (&site, obj) in &state.heap {
        let kind = match obj.kind {
            ObjKind::Object => AllocKind::Object,
            ObjKind::Array => AllocKind::Array,
            ObjKind::TypedArray(kind) => AllocKind::TypedArray(kind),
            ObjKind::Instance(class) => {
                instances.entry(class).or_default().push(obj);
                AllocKind::Instance(class)
            }
            ObjKind::Function(_) => AllocKind::Function,
            ObjKind::Class(_) => AllocKind::Class,
            ObjKind::Prototype(_) | ObjKind::Namespace(_) => continue,
            ObjKind::Opaque => AllocKind::Other,
        };
        let escape = match obj.escape {
            interp::Escape::None => Escape::None,
            interp::Escape::Known => Escape::Known,
            interp::Escape::Unknown => Escape::Unknown,
        };
        let stable = !obj.dynamic
            && !obj.deleted
            && obj.added.is_empty()
            && obj.escape != interp::Escape::Unknown;
        facts.set_allocation(
            site,
            Allocation {
                kind,
                escape: Fact::proven(escape),
                stable_shape: Fact::proven(stable),
            },
        );
    }
    for &class in world.classes.keys() {
        let sites = instances.get(&class).cloned().unwrap_or_default();
        let fields = crate::shapes::definite_fields(world, class);
        let mut shape = ClassShape {
            fields,
            ..ClassShape::default()
        };
        let mut known = true;
        for obj in &sites {
            for key in &obj.order {
                if obj.added.contains(key) && !shape.added_later.contains(key) {
                    shape.added_later.push(key.clone());
                }
            }
            shape.deletes |= obj.deleted;
            shape.dynamic |= obj.dynamic;
            known &= obj.escape != interp::Escape::Unknown;
        }
        let trust = if known && !interp::class_open(world, state, class) {
            Trust::Proven
        } else {
            Trust::Unknown
        };
        if trust == Trust::Unknown {
            shape.deletes = true;
            shape.dynamic = true;
        }
        facts.set_class_shape(
            class,
            Fact {
                value: shape,
                trust,
            },
        );
    }
    facts
}

/// The fact for a value, with object types filled in from the heap.
fn materialize(value: &Val, state: &State) -> Fact<Type> {
    let fact = value.fact();
    if fact.trust == Trust::Unknown {
        return fact;
    }
    Fact {
        value: refine(&fact.value, &value.pts, state, MAX_TYPE_DEPTH),
        trust: fact.trust,
    }
}

fn refine(ty: &Type, pts: &Pts, state: &State, depth: usize) -> Type {
    if depth == 0 {
        return Type::Unknown;
    }
    match ty {
        Type::Union(members) => Type::union(
            members
                .iter()
                .map(|member| refine(member, pts, state, depth)),
        ),
        Type::Object(shape) if shape.properties.is_empty() && !shape.exact => {
            object_shape(pts, state, depth).unwrap_or_else(|| ty.clone())
        }
        Type::Array(element) if **element == Type::Unknown => array_element(pts, state, depth)
            .map_or_else(
                || ty.clone(),
                |element| Type::Array(std::sync::Arc::new(element)),
            ),
        other => other.clone(),
    }
}

fn object_shape(pts: &Pts, state: &State, depth: usize) -> Option<Type> {
    let sites: Vec<&interp::Obj> = pts
        .sites()?
        .iter()
        .filter_map(|site| state.heap.get(site))
        .filter(|obj| obj.kind == ObjKind::Object)
        .collect();
    if sites.is_empty()
        || sites
            .iter()
            .any(|obj| obj.dynamic || obj.escape == interp::Escape::Unknown)
    {
        return None;
    }
    let mut keys: Vec<String> = Vec::new();
    for obj in &sites {
        for key in &obj.order {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
    }
    let exact = sites.iter().all(|obj| {
        !obj.deleted
            && obj.added.is_empty()
            && keys
                .iter()
                .all(|key| obj.present.contains(key) && obj.props.contains_key(key))
    });
    let properties = keys
        .into_iter()
        .map(|key| {
            let mut value = Val::never();
            for obj in &sites {
                match obj.props.get(&key) {
                    Some(prop) if obj.present.contains(&key) => value = value.join(prop),
                    Some(prop) => value = value.join(prop).join(&Val::proven(Type::Undefined)),
                    None => value = value.join(&Val::proven(Type::Undefined)),
                }
            }
            let fact = value.fact();
            let ty = if fact.trust == Trust::Proven {
                refine(&fact.value, &value.pts, state, depth - 1)
            } else {
                Type::Unknown
            };
            (key, ty)
        })
        .collect();
    Some(Type::Object(std::sync::Arc::new(Shape {
        properties,
        exact,
    })))
}

fn array_element(pts: &Pts, state: &State, depth: usize) -> Option<Type> {
    let sites: Vec<&interp::Obj> = pts
        .sites()?
        .iter()
        .filter_map(|site| state.heap.get(site))
        .filter(|obj| obj.kind == ObjKind::Array)
        .collect();
    if sites.is_empty()
        || sites
            .iter()
            .any(|obj| obj.dynamic || obj.escape == interp::Escape::Unknown)
    {
        return None;
    }
    if sites.iter().any(|obj| obj.holey) {
        // A hole reads as `undefined`, or as whatever a prototype holds at that index.
        return Some(Type::Unknown);
    }
    let mut element = Val::never();
    for obj in &sites {
        element = element.join(&obj.elem);
    }
    let fact = element.fact();
    if fact.trust != Trust::Proven {
        return Some(Type::Unknown);
    }
    Some(refine(&fact.value, &element.pts, state, depth - 1))
}

/// Call-site completeness, per-expression and per-loop effects, points-to sets and
/// reachability.
fn usage_facts(
    facts: &mut Facts,
    world: &World<'_>,
    results: &Results,
    graph: &crate::facts::CallGraph,
    summaries: &FxHashMap<NodeRef, (Effects, Effects)>,
) {
    let with_callees = |proven: Effects, declared: Effects, callees: &[NodeRef]| {
        callees
            .iter()
            .fold((proven, declared), |(proven, declared), callee| {
                let (callee_proven, callee_declared) = summaries
                    .get(callee)
                    .copied()
                    .unwrap_or((Effects::ANY, Effects::ANY));
                (
                    union(proven, callee_proven),
                    union(declared, callee_declared),
                )
            })
    };
    let mut pending_loops = Vec::new();
    let mut open_sites = FxHashSet::default();
    let mut reached = FxHashSet::default();
    let mut skipped = Vec::new();
    let mut volatile = FxHashSet::default();
    for (&unit, record) in &results.output.units {
        open_sites.extend(record.open_sites.iter().copied());
        // Visits of one expression are merged (loops revisit them).
        let mut visits: Vec<&interp::ExprVisit> = record.expr_effects.iter().collect();
        visits.sort_unstable_by_key(|visit| visit.node);
        let mut callees: Vec<NodeRef> = Vec::new();
        for group in visits.chunk_by(|a, b| a.node == b.node) {
            let (mut proven, mut declared) = (Effects::PURE, Effects::PURE);
            let mut is_volatile = false;
            callees.clear();
            for visit in group {
                proven = union(proven, visit.proven);
                declared = union(declared, visit.declared);
                // At expression level, writes to locals and reads of mutable locals count.
                proven.writes |= visit.local_writes;
                declared.writes |= visit.local_writes;
                proven.reads |= visit.shared_reads;
                declared.reads |= visit.shared_reads;
                is_volatile |= visit.volatile;
                let (start, end) = (visit.edges.0 as usize, visit.edges.1 as usize);
                callees.extend(record.edges[start..end].iter().map(|edge| edge.callee));
            }
            callees.sort_unstable();
            callees.dedup();
            let (proven, declared) = with_callees(proven, declared, &callees);
            let node = NodeRef {
                module: unit.module,
                node: group[0].node,
            };
            facts.set_expr_effects(node, proven, declared);
            if is_volatile {
                volatile.insert(node);
            }
        }
        let module = world.module(unit.module);
        for (&node, record) in &record.loops {
            let (mut proven, mut declared) = (record.proven, record.declared);
            proven.writes |= record.shared_writes;
            declared.writes |= record.shared_writes;
            let effects = with_callees(proven, declared, &record.callees);
            let symbols = |symbols: &[SymbolId]| -> Vec<SymbolRef> {
                symbols
                    .iter()
                    .map(|&symbol| module.symbol(symbol))
                    .collect()
            };
            let induction = record
                .induction
                .iter()
                .filter_map(|(symbol, value)| {
                    let fact = value.fact();
                    let number = fact.value.as_number()?;
                    Some((
                        module.symbol(*symbol),
                        Fact {
                            value: number,
                            trust: fact.trust,
                        },
                    ))
                })
                .collect();
            let written = symbols(&record.written);
            pending_loops.push((
                node,
                LoopFacts {
                    effects,
                    written,
                    length_changed: symbols(&record.length_changed),
                    induction,
                    invariant: Vec::new(),
                },
            ));
        }
        reached.extend(record.reached.iter().copied());
        skipped.extend(record.skipped.iter().copied());
    }
    // Effects first: invariance needs them.
    for (node, mut loop_facts) in pending_loops {
        loop_facts.invariant = invariant_expressions(facts, world, node, &loop_facts, &volatile);
        facts.set_loop_facts(node, loop_facts);
    }
    facts.set_open_sites(open_sites);
    for (&node, value) in &results.output.nodes {
        if let Some(sites) = value.pts.sites()
            && !sites.is_empty()
        {
            facts.set_points_to(node, sites.to_vec());
        }
    }

    // Units run if something reachable calls them, or code that doesn't record calls can:
    // module bodies, class initialisers, and any unit whose function object escapes.
    let state = &results.state;
    let mut live: FxHashSet<NodeRef> = FxHashSet::default();
    let mut pending: Vec<NodeRef> = world
        .unit_order
        .iter()
        .copied()
        .filter(|&unit| {
            let info = world.unit(unit);
            !matches!(info.body, Body::Function(_) | Body::Arrow(_))
                || state
                    .heap
                    .get(&unit)
                    .is_none_or(|obj| obj.escape != interp::Escape::None)
                || interp::unit_open(world, state, info)
        })
        .collect();
    while let Some(unit) = pending.pop() {
        if !live.insert(unit) {
            continue;
        }
        pending.extend(graph.edges_from(unit).map(|edge| edge.callee));
    }
    // Dead code: unreachable units, and statements no reachable path got to, with everything
    // inside them (node ids are preorder, so a node's descendants follow it).
    let mut roots: Vec<NodeRef> = world
        .unit_order
        .iter()
        .copied()
        .filter(|unit| {
            !live.contains(unit)
                && matches!(world.unit(*unit).body, Body::Function(_) | Body::Arrow(_))
        })
        .collect();
    roots.extend(
        skipped
            .into_iter()
            .filter(|statement| !reached.contains(statement)),
    );
    let mut unreachable = FxHashSet::default();
    for root in roots {
        if unreachable.contains(&root) {
            continue;
        }
        let module = world.module(root.module);
        let nodes = module.semantic.nodes();
        let span = oxc_span::GetSpan::span(&nodes.kind(root.node));
        unreachable.insert(root);
        for index in root.node.index() + 1..nodes.len() {
            let id = NodeId::from_usize(index);
            let inner = oxc_span::GetSpan::span(&nodes.kind(id));
            if !(span.start <= inner.start && inner.end <= span.end) {
                break;
            }
            unreachable.insert(module.node(id));
        }
    }
    facts.set_unreachable(unreachable);
}

/// Either effect set.
fn union(a: Effects, b: Effects) -> Effects {
    Effects {
        reads: a.reads || b.reads,
        writes: a.writes || b.writes,
        allocates: a.allocates || b.allocates,
        throws: a.throws || b.throws,
        calls_unknown: a.calls_unknown || b.calls_unknown,
    }
}

/// The largest loop-invariant expressions of the loop `node` (see [`LoopFacts::invariant`]).
/// Expressions in the loop `node` that give the same value on every iteration and can be
/// evaluated once, before the first iteration's body, after the loop's first test passes.
fn invariant_expressions(
    facts: &Facts,
    world: &World<'_>,
    node: NodeRef,
    loop_facts: &LoopFacts,
    volatile: &FxHashSet<NodeRef>,
) -> Vec<NodeRef> {
    use oxc_ast::AstKind;
    use oxc_span::GetSpan;
    let module = world.module(node.module);
    let nodes = module.semantic.nodes();
    let scoping = module.semantic.scoping();
    let loop_kind = nodes.kind(node.node);
    let loop_span = loop_kind.span();
    // Only the parts run on every iteration, before anything conditional: the test and the
    // body (not a `for` loop's init or update, nor a `for-in`/`of` loop's head).
    let regions: Vec<Span> = match loop_kind {
        AstKind::ForStatement(stmt) => stmt
            .test
            .iter()
            .map(GetSpan::span)
            .chain([stmt.body.span()])
            .collect(),
        AstKind::WhileStatement(stmt) => vec![stmt.test.span(), stmt.body.span()],
        AstKind::DoWhileStatement(stmt) => vec![stmt.body.span(), stmt.test.span()],
        AstKind::ForInStatement(stmt) => vec![stmt.body.span()],
        AstKind::ForOfStatement(stmt) => vec![stmt.body.span()],
        _ => return Vec::new(),
    };
    let loop_writes = loop_facts.effects.0.writes;
    // Node ids are preorder: the loop's descendants follow it.
    let inside: Vec<_> = (node.node.index() + 1..nodes.len())
        .map(NodeId::from_usize)
        .take_while(|&id| {
            let span = nodes.kind(id).span();
            loop_span.start <= span.start && span.end <= loop_span.end
        })
        .collect();
    let mut variant_reads: Vec<Span> = Vec::new();
    for &id in &inside {
        match nodes.kind(id) {
            AstKind::IdentifierReference(ident) => {
                let written = ident
                    .reference_id
                    .get()
                    .and_then(|reference| scoping.get_reference(reference).symbol_id())
                    .is_some_and(|symbol| loop_facts.written.contains(&module.symbol(symbol)));
                if written {
                    variant_reads.push(ident.span);
                }
            }
            // An early exit or a suspension can make what follows conditional (narrowed by
            // the exit's guard) or let other code run between iterations.
            AstKind::BreakStatement(_)
            | AstKind::ContinueStatement(_)
            | AstKind::ReturnStatement(_)
            | AstKind::ThrowStatement(_)
            | AstKind::AwaitExpression(_)
            | AstKind::YieldExpression(_)
                if !in_nested_function(nodes, id, node.node) =>
            {
                return Vec::new();
            }
            _ => {}
        }
    }
    let mut found: Vec<(NodeRef, Span)> = Vec::new();
    for &id in &inside {
        let kind = nodes.kind(id);
        if !matches!(
            kind,
            AstKind::BinaryExpression(_)
                | AstKind::UnaryExpression(_)
                | AstKind::LogicalExpression(_)
                | AstKind::ConditionalExpression(_)
                | AstKind::StaticMemberExpression(_)
                | AstKind::ComputedMemberExpression(_)
                | AstKind::CallExpression(_)
                | AstKind::TemplateLiteral(_)
        ) {
            continue;
        }
        let span = kind.span();
        if !regions
            .iter()
            .any(|region| region.start <= span.start && span.end <= region.end)
            || found
                .iter()
                .any(|(_, outer)| outer.start <= span.start && span.end <= outer.end)
        {
            continue;
        }
        let expression = module.node(id);
        let effects = facts.expr_effects(expression);
        if effects.trust != Trust::Proven
            || !effects.value.is_side_effect_free()
            || effects.value.allocates
            || (effects.value.reads && loop_writes)
            || volatile.contains(&expression)
            || variant_reads
                .iter()
                .any(|read| span.start <= read.start && read.end <= span.end)
            || !unconditional(nodes, id, node.node, span)
        {
            continue;
        }
        found.push((expression, span));
    }
    found.into_iter().map(|(node, _)| node).collect()
}

/// Whether `id` is inside a function nested in `outer`.
fn in_nested_function(nodes: &oxc_semantic::AstNodes<'_>, id: NodeId, outer: NodeId) -> bool {
    use oxc_ast::AstKind;
    nodes
        .ancestor_ids(id)
        .take_while(|&ancestor| ancestor != outer)
        .any(|ancestor| {
            matches!(
                nodes.kind(ancestor),
                AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) | AstKind::Class(_)
            )
        })
}

/// Whether the expression `id` (spanning `span`) runs whenever its loop `outer`'s iteration
/// gets that far, in a position where its value alone matters: no conditional, nested loop
/// or function between them, and not a callee (its `this` would be lost) or a write target.
fn unconditional(
    nodes: &oxc_semantic::AstNodes<'_>,
    id: NodeId,
    outer: NodeId,
    span: Span,
) -> bool {
    use oxc_ast::AstKind;
    use oxc_span::GetSpan;
    match nodes.parent_kind(id) {
        AstKind::CallExpression(call) if call.callee.span() == span => return false,
        AstKind::NewExpression(call) if call.callee.span() == span => return false,
        AstKind::TaggedTemplateExpression(tagged) if tagged.tag.span() == span => return false,
        AstKind::UnaryExpression(unary)
            if unary.operator == oxc_syntax::operator::UnaryOperator::Delete =>
        {
            return false;
        }
        _ => {}
    }
    let mut child = id;
    for ancestor in nodes.ancestor_ids(id) {
        if ancestor == outer {
            return true;
        }
        let kind = nodes.kind(ancestor);
        let child_span = nodes.kind(child).span();
        let conditional = match kind {
            AstKind::LogicalExpression(logical) => logical.right.span() == child_span,
            AstKind::ConditionalExpression(conditional) => conditional.test.span() != child_span,
            AstKind::IfStatement(stmt) => stmt.test.span() != child_span,
            AstKind::SwitchStatement(stmt) => stmt.discriminant.span() != child_span,
            AstKind::AssignmentExpression(assignment) => {
                assignment.operator.is_logical() || assignment.left.span() == child_span
            }
            AstKind::UpdateExpression(_)
            | AstKind::ChainExpression(_)
            | AstKind::TryStatement(_)
            | AstKind::ForStatement(_)
            | AstKind::ForInStatement(_)
            | AstKind::ForOfStatement(_)
            | AstKind::WhileStatement(_)
            | AstKind::DoWhileStatement(_)
            | AstKind::Function(_)
            | AstKind::ArrowFunctionExpression(_)
            | AstKind::Class(_)
            | AstKind::FormalParameter(_)
            | AstKind::AssignmentPattern(_)
            | AstKind::AssignmentTargetWithDefault(_)
            | AstKind::ArrayAssignmentTarget(_)
            | AstKind::ObjectAssignmentTarget(_) => true,
            _ => false,
        };
        if conditional {
            return false;
        }
        child = ancestor;
    }
    false
}
