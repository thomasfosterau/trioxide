//! Turns the interpreter's tables into [`Facts`].

use oxc_span::Span;
use oxc_syntax::symbol::SymbolId;
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::{ModuleId, NodeId, NodeRef, SymbolRef};
use trioxide_types::{NumberType, Shape, Type};

use crate::{
    AnalysisOptions, callgraph,
    facts::{
        AllocKind, Allocation, ClassShape, Effects, Escape, Fact, Facts, FunctionInfo, Induction,
        LoopFacts, Trust,
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
    if !results.converged {
        // Nothing the last round computed can be trusted.
        for (unit, info) in functions(world, results, &world.unit_order) {
            facts.set_function(unit, info);
        }
        return facts;
    }
    // The rest doesn't need the node facts: work it out alongside.
    // SAFETY: the world and the results are only read until the scope below ends.
    let shared = unsafe { crate::threads::Shared::new((world, results)) };
    let (modules, (graph, effects), (symbols, (allocations, shapes, packed))) =
        std::thread::scope(|scope| {
            let calls = scope.spawn(|| {
                let (world, results) = shared.get();
                let graph = callgraph::build(world, results);
                let effects = callgraph::effects(&graph, results, &results.state);
                (graph, effects)
            });
            let heap = scope.spawn(|| {
                let (world, results) = shared.get();
                let symbols: Vec<_> = results
                    .state
                    .symbols
                    .iter()
                    .map(|(&symbol, value)| (symbol, materialize(value, &results.state)))
                    .collect();
                (symbols, heap_facts(world, &results.state))
            });
            let modules = node_types(world, results, crate::threads::count(options));
            (
                modules,
                calls.join().expect("summarising panicked"),
                heap.join().expect("summarising panicked"),
            )
        });
    for (module, types, locals, functions) in modules {
        for (unit, info) in functions {
            facts.set_function(unit, info);
        }
        facts.set_module_types(module, types);
        for (symbol, fact) in locals {
            facts.set_symbol_ty(symbol, fact);
        }
    }
    for (symbol, fact) in symbols {
        facts.set_symbol_ty(symbol, fact);
    }
    // An access two units recorded (a nested unit's, say) is only as trusted as the weaker.
    let mut in_bounds: FxHashMap<NodeRef, Trust> = FxHashMap::default();
    for output in results.outputs.values() {
        for (&node, &trust) in &output.in_bounds {
            in_bounds
                .entry(node)
                .and_modify(|old| *old = (*old).min(trust))
                .or_insert(trust);
        }
    }
    for (node, trust) in in_bounds {
        facts.set_in_bounds(node, trust);
    }
    let summaries: FxHashMap<NodeRef, (Effects, Effects)> = effects.into_iter().collect();
    for (&unit, &(proven, declared)) in &summaries {
        facts.set_effect_summaries(unit, proven, declared);
    }
    usage_facts(
        &mut facts,
        world,
        results,
        &graph,
        &summaries,
        crate::threads::count(options),
    );
    facts.set_call_graph(graph);

    for (site, allocation) in allocations {
        facts.set_allocation(site, allocation);
    }
    for (class, shape) in shapes {
        facts.set_class_shape(class, shape);
    }
    for site in packed {
        facts.set_packed(site);
    }
    facts.set_patched(interp::patched_builtins(world));
    facts.set_unobserved_classes(crate::observe::unobserved_classes(world, results));
    facts
}

/// What the heap says about each allocation site and class, and the arrays that never have
/// holes.
type HeapFacts = (
    Vec<(NodeRef, Allocation)>,
    Vec<(NodeRef, Fact<ClassShape>)>,
    Vec<NodeRef>,
);

/// The allocation facts of every site, the shape of every class and the packed array sites.
fn heap_facts(world: &World<'_>, state: &State) -> HeapFacts {
    let mut allocations = Vec::with_capacity(state.heap.len());
    let mut shapes = Vec::with_capacity(world.classes.len());
    let mut instances: FxHashMap<NodeRef, Vec<&interp::Obj>> = FxHashMap::default();
    let mut packed = Vec::new();
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
        if obj.kind == ObjKind::Array
            && !obj.holey
            && !obj.poisoned
            && !obj.given_up
            && obj.escape != interp::Escape::Unknown
        {
            packed.push(site);
        }
        let stable = !obj.dynamic
            && !obj.deleted
            && obj.added.is_empty()
            && obj.escape != interp::Escape::Unknown;
        allocations.push((
            site,
            Allocation {
                kind,
                escape: Fact::proven(escape),
                stable_shape: Fact::proven(stable),
            },
        ));
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
        shapes.push((
            class,
            Fact {
                value: shape,
                trust,
            },
        ));
    }
    (allocations, shapes, packed)
}

/// The fact for a value, with object types filled in from the heap.
/// What each of `units` is, and whether unknown code may call it.
fn functions(
    world: &World<'_>,
    results: &Results,
    units: &[NodeRef],
) -> Vec<(NodeRef, FunctionInfo)> {
    units
        .iter()
        .map(|&unit| {
            let info = world.unit(unit);
            (
                unit,
                FunctionInfo {
                    name: info.name.clone(),
                    span: info.span,
                    open: interp::unit_open(world, &results.state, info),
                },
            )
        })
        .collect()
}

/// What summarising one module found: its node facts, its private locals' facts and its
/// units.
type ModuleFacts = (
    ModuleId,
    FxHashMap<NodeId, Fact<Type>>,
    Vec<(SymbolRef, Fact<Type>)>,
    Vec<(NodeRef, FunctionInfo)>,
);

/// Every module's node facts, made on up to `threads` threads: the values the module's units
/// recorded (joined where two units recorded the same node), materialized; the facts of its
/// private locals; and what its units are.
fn node_types(world: &World<'_>, results: &Results, threads: usize) -> Vec<ModuleFacts> {
    // SAFETY: the world and the results are only read until `map_ranges` returns.
    let shared = unsafe { crate::threads::Shared::new((world, results)) };
    crate::threads::map_ranges(world.modules.len(), threads, |range| {
        let (world, results) = shared.get();
        let mut out = Vec::with_capacity(range.len());
        for module in &world.modules[range] {
            let outputs: Vec<&interp::Output> = world
                .module_units(module.id)
                .iter()
                .filter_map(|unit| results.outputs.get(unit))
                .collect();
            // Nearly every node is recorded by one unit: materialize its value straight away,
            // and redo the few recorded more than once from the join of their values.
            let mut types: FxHashMap<NodeId, Fact<Type>> = FxHashMap::with_capacity_and_hasher(
                outputs.iter().map(|output| output.nodes.len()).sum(),
                rustc_hash::FxBuildHasher,
            );
            let mut repeated: Vec<NodeRef> = Vec::new();
            for output in &outputs {
                for (&node, value) in &output.nodes {
                    match types.entry(node.node) {
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(materialize(value, &results.state));
                        }
                        std::collections::hash_map::Entry::Occupied(_) => repeated.push(node),
                    }
                }
            }
            repeated.sort_unstable();
            repeated.dedup();
            for node in repeated {
                let mut values = outputs.iter().filter_map(|output| output.nodes.get(&node));
                let first = values.next().expect("a repeated node was recorded").clone();
                let joined = values.fold(first, |joined, value| joined.join(value));
                types.insert(node.node, materialize(&joined, &results.state));
            }
            // Private locals' values never reached the shared tables: their units have them.
            let locals = world
                .module_units(module.id)
                .iter()
                .filter_map(|unit| results.outputs.get(unit))
                .flat_map(|output| &output.symbols)
                .map(|(&symbol, value)| (symbol, materialize(value, &results.state)))
                .collect();
            let functions = functions(world, results, world.module_units(module.id));
            out.push((module.id, types, locals, functions));
        }
        out
    })
}

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
        // An instance whose prototype may have been replaced (a `__proto__` write or
        // `Object.setPrototypeOf` poisons it; unknown code may do it to what it holds when the
        // program can) is just an object.
        Type::Instance(_)
            if pts.sites().is_some_and(|sites| {
                sites.iter().any(|site| {
                    state.heap.get(site).is_some_and(|obj| {
                        matches!(obj.kind, ObjKind::Instance(_))
                            && (obj.poisoned
                                || (state.reprototypes && obj.escape == interp::Escape::Unknown))
                    })
                })
            }) =>
        {
            Type::empty_object()
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
        .filter_map(|site| state.heap.get(site).map(|obj| &**obj))
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
        .filter_map(|site| state.heap.get(site).map(|obj| &**obj))
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

/// What one module's units recorded, summarised.
struct ModuleUsage {
    module: ModuleId,
    expr_effects: FxHashMap<NodeId, (Effects, Effects)>,
    points_to: FxHashMap<NodeId, Vec<NodeRef>>,
    volatile: Vec<NodeRef>,
    /// Loop facts, without their invariants yet.
    loops: Vec<(NodeRef, LoopFacts)>,
    open_sites: Vec<NodeRef>,
    reached: Vec<NodeRef>,
    skipped: Vec<NodeRef>,
}

/// Call-site completeness, per-expression and per-loop effects, points-to sets and
/// reachability.
fn usage_facts(
    facts: &mut Facts,
    world: &World<'_>,
    results: &Results,
    graph: &crate::facts::CallGraph,
    summaries: &FxHashMap<NodeRef, (Effects, Effects)>,
    threads: usize,
) {
    // SAFETY: these are only read until `map_ranges` returns.
    let shared = unsafe { crate::threads::Shared::new((world, results, graph, summaries)) };
    let modules = crate::threads::map_ranges(world.modules.len(), threads, |range| {
        let (world, results, graph, summaries) = shared.get();
        world.modules[range]
            .iter()
            .map(|module| module_usage(world, results, graph, summaries, module.id))
            .collect()
    });
    let mut pending_loops = Vec::new();
    let mut open_sites = FxHashSet::default();
    let mut reached = FxHashSet::default();
    let mut skipped = Vec::new();
    let mut volatile = FxHashSet::default();
    for usage in modules {
        facts.set_module_expr_effects(usage.module, usage.expr_effects);
        facts.set_module_points_to(usage.module, usage.points_to);
        volatile.extend(usage.volatile);
        pending_loops.extend(usage.loops);
        open_sites.extend(usage.open_sites);
        reached.extend(usage.reached);
        skipped.extend(usage.skipped);
    }
    // Effects first: invariance needs them.
    let invariants = {
        // SAFETY: these are only read until `map_ranges` returns.
        let shared =
            unsafe { crate::threads::Shared::new((&*facts, world, &volatile, &pending_loops)) };
        crate::threads::map_ranges(pending_loops.len(), threads, |range| {
            let (facts, world, volatile, pending_loops) = shared.get();
            pending_loops[range]
                .iter()
                .map(|(node, loop_facts)| {
                    invariant_expressions(facts, world, *node, loop_facts, volatile)
                })
                .collect()
        })
    };
    for ((node, mut loop_facts), invariant) in pending_loops.into_iter().zip(invariants) {
        loop_facts.invariant = invariant;
        facts.set_loop_facts(node, loop_facts);
    }
    facts.set_open_sites(open_sites);

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
/// [`ModuleUsage`] for `module`.
fn module_usage(
    world: &World<'_>,
    results: &Results,
    graph: &crate::facts::CallGraph,
    summaries: &FxHashMap<NodeRef, (Effects, Effects)>,
    module: ModuleId,
) -> ModuleUsage {
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
    let mut usage = ModuleUsage {
        module,
        expr_effects: FxHashMap::default(),
        points_to: FxHashMap::default(),
        volatile: Vec::new(),
        loops: Vec::new(),
        open_sites: Vec::new(),
        reached: Vec::new(),
        skipped: Vec::new(),
    };
    let info = world.module(module);
    let mut values: FxHashMap<NodeId, Pts> = FxHashMap::default();
    let mut callees: Vec<NodeRef> = Vec::new();
    for unit in world.module_units(module) {
        let Some(output) = results.outputs.get(unit) else {
            continue;
        };
        // A node more than one unit records (rare) may be any of their objects.
        for (&node, value) in &output.nodes {
            if value.pts.is_none() {
                continue;
            }
            values
                .entry(node.node)
                .and_modify(|pts| *pts = pts.join(&value.pts))
                .or_insert_with(|| value.pts.clone());
        }
        let Some(record) = output.units.get(unit) else {
            continue;
        };
        usage.open_sites.extend(record.open_sites.iter().copied());
        usage.reached.extend(record.reached.iter().copied());
        usage.skipped.extend(record.skipped.iter().copied());
        // Visits of one expression are merged (loops revisit them).
        let mut visits: Vec<&interp::ExprVisit> = record.expr_effects.iter().collect();
        visits.sort_unstable_by_key(|visit| visit.node);
        usage.expr_effects.reserve(visits.len());
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
            usage
                .expr_effects
                .insert(group[0].node, with_callees(proven, declared, &callees));
            if is_volatile {
                usage.volatile.push(info.node(group[0].node));
            }
        }
        for (&node, record) in &record.loops {
            let (mut proven, mut declared) = (record.proven, record.declared);
            proven.writes |= record.shared_writes;
            declared.writes |= record.shared_writes;
            let effects = with_callees(proven, declared, &record.callees);
            let symbols = |symbols: &[SymbolId]| -> Vec<SymbolRef> {
                symbols.iter().map(|&symbol| info.symbol(symbol)).collect()
            };
            let ranges: Vec<(SymbolId, Fact<NumberType>)> = record
                .induction
                .iter()
                .filter_map(|(symbol, value)| {
                    let fact = value.fact();
                    let number = fact.value.as_number()?;
                    Some((
                        *symbol,
                        Fact {
                            value: number,
                            trust: fact.trust,
                        },
                    ))
                })
                .collect();
            let induction = induction_variables(world, node, &ranges);
            // Arrays resized here or by any unit it calls, transitively.
            let mut resized: FxHashSet<NodeRef> = record.resized.iter().copied().collect();
            let mut resizes_untracked = record.resizes_untracked || effects.0.calls_unknown;
            let mut pending: Vec<NodeRef> = record.callees.clone();
            let mut seen: FxHashSet<NodeRef> = pending.iter().copied().collect();
            while let Some(callee) = pending.pop() {
                match results.record(callee) {
                    Some(callee) => {
                        resized.extend(callee.resized.iter().copied());
                        resizes_untracked |= callee.resizes_untracked;
                    }
                    None => resizes_untracked = true,
                }
                for edge in graph.edges_from(callee) {
                    if seen.insert(edge.callee) {
                        pending.push(edge.callee);
                    }
                }
            }
            let mut resized: Vec<NodeRef> = resized.into_iter().collect();
            resized.sort_unstable();
            usage.loops.push((
                node,
                LoopFacts {
                    effects,
                    written: symbols(&record.written),
                    length_changed: symbols(&record.length_changed),
                    resized,
                    resizes_untracked,
                    induction,
                    invariant: Vec::new(),
                },
            ));
        }
    }
    usage.points_to = values
        .into_iter()
        .filter_map(|(node, pts)| {
            let sites = pts.sites()?;
            (!sites.is_empty()).then(|| (node, sites.to_vec()))
        })
        .collect();
    usage
}

/// The numeric locals of the loop `node` with their head ranges, minus those declared inside
/// it (a nested loop's, the body's), with their steps.
fn induction_variables(
    world: &World<'_>,
    node: NodeRef,
    ranges: &[(SymbolId, Fact<NumberType>)],
) -> Vec<Induction> {
    use oxc_ast::{
        AstKind,
        ast::{AssignmentOperator, BinaryOperator, Expression, UpdateOperator},
    };
    use oxc_span::GetSpan;
    let module = world.module(node.module);
    let nodes = module.semantic.nodes();
    let scoping = module.semantic.scoping();
    let loop_kind = nodes.kind(node.node);
    let loop_span = loop_kind.span();
    let (head, test, update, body) = match loop_kind {
        AstKind::ForStatement(stmt) => (
            stmt.init.as_ref().map(GetSpan::span),
            stmt.test.as_ref().map(GetSpan::span),
            stmt.update.as_ref().map(GetSpan::span),
            stmt.body.span(),
        ),
        AstKind::WhileStatement(stmt) => (None, Some(stmt.test.span()), None, stmt.body.span()),
        AstKind::DoWhileStatement(stmt) => (None, Some(stmt.test.span()), None, stmt.body.span()),
        AstKind::ForInStatement(stmt) => (Some(stmt.left.span()), None, None, stmt.body.span()),
        AstKind::ForOfStatement(stmt) => (Some(stmt.left.span()), None, None, stmt.body.span()),
        _ => return Vec::new(),
    };
    let within = |inner: Span, outer: Span| outer.start <= inner.start && inner.end <= outer.end;
    // The top-level statements of the body, which run once per iteration unless a
    // `continue` skips them.
    let body_statements: Vec<Span> = match loop_kind {
        AstKind::ForStatement(stmt) => statements_of(&stmt.body),
        AstKind::WhileStatement(stmt) => statements_of(&stmt.body),
        AstKind::DoWhileStatement(stmt) => statements_of(&stmt.body),
        _ => Vec::new(),
    };
    let unit_of = |id: NodeId| {
        nodes.ancestor_ids(id).find(|&ancestor| {
            matches!(
                nodes.kind(ancestor),
                AstKind::Function(_)
                    | AstKind::ArrowFunctionExpression(_)
                    | AstKind::StaticBlock(_)
            )
        })
    };
    let loop_unit = unit_of(node.node);
    if ranges.is_empty() {
        return Vec::new();
    }
    // Node ids are preorder: the loop's descendants follow it.
    let continues = (node.node.index() + 1..nodes.len())
        .map(NodeId::from_usize)
        .take_while(|&id| within(nodes.kind(id).span(), loop_span))
        .any(|id| {
            matches!(nodes.kind(id), AstKind::ContinueStatement(_))
                && within(nodes.kind(id).span(), body)
                && unit_of(id) == loop_unit
        });
    ranges
        .iter()
        .filter_map(|(symbol, range)| {
            let (symbol, range) = (*symbol, range.clone());
            let declared = nodes.kind(scoping.symbol_declaration(symbol)).span();
            if within(declared, loop_span) && !head.is_some_and(|head| within(declared, head)) {
                return None;
            }
            let mut writes = Vec::new();
            let mut tested = false;
            for &reference in scoping.get_resolved_reference_ids(symbol) {
                let reference = scoping.get_reference(reference);
                let at = reference.node_id();
                let span = nodes.kind(at).span();
                if reference.is_write() {
                    if unit_of(at) != loop_unit {
                        // A closure writes it: anything may happen between iterations.
                        writes.push(None);
                    } else if within(span, loop_span) {
                        writes.push(Some(at));
                    }
                }
                if reference.is_read() && test.is_some_and(|test| within(span, test)) {
                    tested = true;
                }
            }
            let step = match writes.as_slice() {
                [Some(at)] => {
                    // The expression the write is the target of.
                    let expression = nodes.parent_id(*at);
                    let span = nodes.kind(expression).span();
                    let once = update == Some(span)
                        || (!continues && body_statements.contains(&span));
                    let constant = |expression: &Expression<'_>| match expression.without_parentheses() {
                        Expression::NumericLiteral(literal) => Some(literal.value),
                        Expression::UnaryExpression(unary)
                            if unary.operator == oxc_ast::ast::UnaryOperator::UnaryNegation =>
                        {
                            match unary.argument.without_parentheses() {
                                Expression::NumericLiteral(literal) => Some(-literal.value),
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    let names = |expression: &Expression<'_>| {
                        matches!(expression.without_parentheses(), Expression::Identifier(ident)
                            if ident.reference_id.get().and_then(|reference| scoping.get_reference(reference).symbol_id()) == Some(symbol))
                    };
                    let step = match nodes.kind(expression) {
                        AstKind::UpdateExpression(update) => Some(match update.operator {
                            UpdateOperator::Increment => 1.0,
                            UpdateOperator::Decrement => -1.0,
                        }),
                        AstKind::AssignmentExpression(assign) => match assign.operator {
                            AssignmentOperator::Addition => constant(&assign.right),
                            AssignmentOperator::Subtraction => constant(&assign.right).map(|c| -c),
                            AssignmentOperator::Assign => match assign.right.without_parentheses() {
                                Expression::BinaryExpression(binary)
                                    if binary.operator == BinaryOperator::Addition =>
                                {
                                    if names(&binary.left) {
                                        constant(&binary.right)
                                    } else if names(&binary.right) {
                                        constant(&binary.left)
                                    } else {
                                        None
                                    }
                                }
                                Expression::BinaryExpression(binary)
                                    if binary.operator == BinaryOperator::Subtraction
                                        && names(&binary.left) =>
                                {
                                    constant(&binary.right).map(|c| -c)
                                }
                                _ => None,
                            },
                            _ => None,
                        },
                        _ => None,
                    };
                    // Only whole steps can be exact.
                    step.filter(|step| once && step.is_finite() && step.fract() == 0.0)
                }
                _ => None,
            };
            Some(Induction {
                symbol: module.symbol(symbol),
                range,
                step,
                counter: step.is_some() && tested,
            })
        })
        .collect()
}

/// The spans of the statements directly in `body` (itself, if it isn't a block), as
/// expression statements' expressions.
fn statements_of(body: &oxc_ast::ast::Statement<'_>) -> Vec<Span> {
    use oxc_ast::ast::Statement;
    use oxc_span::GetSpan;
    let expression = |statement: &Statement<'_>| match statement {
        Statement::ExpressionStatement(statement) => Some(statement.expression.span()),
        _ => None,
    };
    match body {
        Statement::BlockStatement(block) => block.body.iter().filter_map(expression).collect(),
        statement => expression(statement).into_iter().collect(),
    }
}

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
