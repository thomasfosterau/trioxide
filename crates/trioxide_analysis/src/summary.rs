//! Turns the interpreter's tables into [`Facts`].

use oxc_span::Span;
use oxc_syntax::symbol::SymbolId;
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::{ModuleId, NodeRef, SymbolRef};
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
    let mut open_sites = FxHashSet::default();
    let mut reached = FxHashSet::default();
    let mut skipped = Vec::new();
    for (&unit, record) in &results.output.units {
        open_sites.extend(record.open_sites.iter().copied());
        for (&node, expr) in &record.expr_effects {
            let (proven, declared) = with_callees(expr.proven, expr.declared, &expr.callees);
            facts.set_expr_effects(node, proven, declared);
        }
        let module = world.module(unit.module);
        for (&node, record) in &record.loops {
            let effects = with_callees(record.proven, record.declared, &record.callees);
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
            facts.set_loop_facts(
                node,
                LoopFacts {
                    effects,
                    written: symbols(&record.written),
                    length_changed: symbols(&record.length_changed),
                    induction,
                },
            );
        }
        reached.extend(record.reached.iter().copied());
        skipped.extend(record.skipped.iter().copied());
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
    // Dead code: unreachable units, and statements no reachable path got to.
    let mut dead: FxHashMap<ModuleId, Vec<Span>> = FxHashMap::default();
    let mut unreachable = FxHashSet::default();
    for &unit in &world.unit_order {
        if live.contains(&unit) {
            continue;
        }
        let span = match world.unit(unit).body {
            Body::Function(function) => function.span,
            Body::Arrow(arrow) => arrow.span,
            _ => continue,
        };
        dead.entry(unit.module).or_default().push(span);
    }
    for statement in skipped {
        if reached.contains(&statement) {
            continue;
        }
        let module = world.module(statement.module);
        let span = oxc_span::GetSpan::span(&module.semantic.nodes().kind(statement.node));
        dead.entry(statement.module).or_default().push(span);
    }
    for (module_id, spans) in dead {
        let module = world.module(module_id);
        for node in module.semantic.nodes().iter() {
            let span = oxc_span::GetSpan::span(&node.kind());
            if spans
                .iter()
                .any(|dead| dead.start <= span.start && span.end <= dead.end)
            {
                unreachable.insert(module.node(node.id()));
            }
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
