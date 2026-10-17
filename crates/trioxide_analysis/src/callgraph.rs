//! The call graph over code units, its strongly connected components, and effect summaries
//! computed bottom-up over them (PLAN §2 step 4).

use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::NodeRef;

use crate::{
    facts::{CallEdge, CallGraph, Effects, Trust},
    interp::{Escape, Output, State},
    world::World,
};

/// Builds the call graph from the recorded edges.
pub fn build(world: &World<'_>, output: &Output) -> CallGraph {
    let mut edges = Vec::new();
    for &unit in &world.unit_order {
        if let Some(record) = output.units.get(&unit) {
            for edge in &record.edges {
                edges.push(CallEdge {
                    site: edge.site,
                    caller: unit,
                    callee: edge.callee,
                    trust: edge.trust,
                });
            }
        }
    }
    edges.sort_by_key(|edge| (edge.caller, edge.site, edge.callee, edge.trust));
    edges.dedup();
    let sccs = tarjan(&world.unit_order, &edges);
    CallGraph::new(world.unit_order.clone(), edges, sccs)
}

/// Tarjan's algorithm. Components come out callees first (reverse topological order).
fn tarjan(nodes: &[NodeRef], edges: &[CallEdge]) -> Vec<Vec<NodeRef>> {
    let mut successors: FxHashMap<NodeRef, Vec<NodeRef>> = FxHashMap::default();
    for edge in edges {
        successors.entry(edge.caller).or_default().push(edge.callee);
    }
    struct Frame {
        node: NodeRef,
        next: usize,
    }
    let mut index: FxHashMap<NodeRef, usize> = FxHashMap::default();
    let mut low: FxHashMap<NodeRef, usize> = FxHashMap::default();
    let mut on_stack: FxHashSet<NodeRef> = FxHashSet::default();
    let mut stack: Vec<NodeRef> = Vec::new();
    let mut components = Vec::new();
    let mut counter = 0;
    for &root in nodes {
        if index.contains_key(&root) {
            continue;
        }
        let mut frames = vec![Frame { node: root, next: 0 }];
        index.insert(root, counter);
        low.insert(root, counter);
        counter += 1;
        stack.push(root);
        on_stack.insert(root);
        while let Some(frame) = frames.last_mut() {
            let node = frame.node;
            let next = successors.get(&node).and_then(|list| list.get(frame.next)).copied();
            if let Some(successor) = next {
                frame.next += 1;
                if !index.contains_key(&successor) {
                    index.insert(successor, counter);
                    low.insert(successor, counter);
                    counter += 1;
                    stack.push(successor);
                    on_stack.insert(successor);
                    frames.push(Frame { node: successor, next: 0 });
                } else if on_stack.contains(&successor) {
                    let value = low[&node].min(index[&successor]);
                    low.insert(node, value);
                }
                continue;
            }
            frames.pop();
            if let Some(parent) = frames.last() {
                let value = low[&parent.node].min(low[&node]);
                low.insert(parent.node, value);
            }
            if low[&node] == index[&node] {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack.remove(&member);
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                component.sort();
                components.push(component);
            }
        }
    }
    components
}

/// Effect summaries per unit: `(proven, declared)`, each including what callees do.
pub fn effects(
    world: &World<'_>,
    graph: &CallGraph,
    output: &Output,
    state: &State,
) -> FxHashMap<NodeRef, (Effects, Effects)> {
    let mut summaries: FxHashMap<NodeRef, (Effects, Effects)> = FxHashMap::default();
    let local = |unit: NodeRef| -> (Effects, Effects) {
        let Some(record) = output.units.get(&unit) else {
            return (Effects::ANY, Effects::ANY);
        };
        let allocates = record.allocations.iter().any(|site| {
            state.heap.get(site).is_none_or(|obj| obj.escape != Escape::None)
        });
        let mut proven = record.proven;
        let mut declared = record.declared;
        proven.allocates |= allocates;
        declared.allocates |= allocates;
        (proven, declared)
    };
    for component in graph.sccs() {
        let members: FxHashSet<NodeRef> = component.iter().copied().collect();
        let mut proven = Effects::PURE;
        let mut declared = Effects::PURE;
        for &unit in component {
            let (p, d) = local(unit);
            proven = union(proven, p);
            declared = union(declared, d);
            for edge in graph.edges_from(unit) {
                if members.contains(&edge.callee) {
                    continue;
                }
                let (callee_proven, callee_declared) = summaries
                    .get(&edge.callee)
                    .copied()
                    .unwrap_or((Effects::ANY, Effects::ANY));
                if edge.trust == Trust::Proven {
                    proven = union(proven, callee_proven);
                }
                declared = union(declared, callee_declared);
            }
        }
        for &unit in component {
            let _ = world;
            summaries.insert(unit, (proven, declared));
        }
    }
    summaries
}

fn union(a: Effects, b: Effects) -> Effects {
    Effects {
        reads: a.reads || b.reads,
        writes: a.writes || b.writes,
        allocates: a.allocates || b.allocates,
        throws: a.throws || b.throws,
        calls_unknown: a.calls_unknown || b.calls_unknown,
    }
}
