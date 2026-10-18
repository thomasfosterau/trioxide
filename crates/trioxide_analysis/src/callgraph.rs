//! The call graph over code units, its strongly connected components, and effect summaries
//! computed bottom-up over them (PLAN §2 step 4).

use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::NodeRef;

use crate::{
    facts::{CallEdge, CallGraph, Effects, Trust},
    interp::{Escape, Results, State},
    world::World,
};

/// Builds the call graph from the recorded edges.
pub fn build(world: &World<'_>, results: &Results) -> CallGraph {
    let mut edges = Vec::new();
    for &unit in &world.unit_order {
        if let Some(record) = results.record(unit) {
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
    struct Frame {
        node: usize,
        next: usize,
    }
    const UNVISITED: usize = usize::MAX;
    // Dense ids: the nodes, then any callee that isn't one, in order of appearance.
    let mut ids: FxHashMap<NodeRef, usize> =
        FxHashMap::with_capacity_and_hasher(nodes.len(), rustc_hash::FxBuildHasher);
    let mut names: Vec<NodeRef> = Vec::with_capacity(nodes.len());
    let mut id_of = |node: NodeRef, names: &mut Vec<NodeRef>| {
        *ids.entry(node).or_insert_with(|| {
            names.push(node);
            names.len() - 1
        })
    };
    for &node in nodes {
        id_of(node, &mut names);
    }
    let pairs: Vec<(usize, usize)> = edges
        .iter()
        .map(|edge| {
            (
                id_of(edge.caller, &mut names),
                id_of(edge.callee, &mut names),
            )
        })
        .collect();
    // Successors in edge order, as compressed rows.
    let mut starts = vec![0usize; names.len() + 1];
    for &(caller, _) in &pairs {
        starts[caller + 1] += 1;
    }
    for at in 1..starts.len() {
        starts[at] += starts[at - 1];
    }
    let mut filled = starts.clone();
    let mut successors = vec![0usize; pairs.len()];
    for &(caller, callee) in &pairs {
        successors[filled[caller]] = callee;
        filled[caller] += 1;
    }
    let mut index = vec![UNVISITED; names.len()];
    let mut low = vec![0usize; names.len()];
    let mut on_stack = vec![false; names.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut components = Vec::new();
    let mut counter = 0;
    for root in 0..nodes.len() {
        if index[root] != UNVISITED {
            continue;
        }
        let mut frames = vec![Frame {
            node: root,
            next: 0,
        }];
        index[root] = counter;
        low[root] = counter;
        counter += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(frame) = frames.last_mut() {
            let node = frame.node;
            let at = starts[node] + frame.next;
            if at < starts[node + 1] {
                let successor = successors[at];
                frame.next += 1;
                if index[successor] == UNVISITED {
                    index[successor] = counter;
                    low[successor] = counter;
                    counter += 1;
                    stack.push(successor);
                    on_stack[successor] = true;
                    frames.push(Frame {
                        node: successor,
                        next: 0,
                    });
                } else if on_stack[successor] {
                    low[node] = low[node].min(index[successor]);
                }
                continue;
            }
            frames.pop();
            if let Some(parent) = frames.last() {
                low[parent.node] = low[parent.node].min(low[node]);
            }
            if low[node] == index[node] {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component.push(names[member]);
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
    graph: &CallGraph,
    results: &Results,
    state: &State,
) -> FxHashMap<NodeRef, (Effects, Effects)> {
    let mut summaries: FxHashMap<NodeRef, (Effects, Effects)> = FxHashMap::default();
    let local = |unit: NodeRef| -> (Effects, Effects) {
        let Some(record) = results.record(unit) else {
            return (Effects::ANY, Effects::ANY);
        };
        let allocates = record.allocations.iter().any(|site| {
            state
                .heap
                .get(site)
                .is_none_or(|obj| obj.escape != Escape::None)
        });
        let mut proven = record.proven;
        let mut declared = record.declared;
        proven.allocates |= allocates;
        declared.allocates |= allocates;
        (proven, declared)
    };
    summaries.reserve(graph.units().len());
    for component in graph.sccs() {
        let members: FxHashSet<NodeRef> = if component.len() > 1 {
            component.iter().copied().collect()
        } else {
            FxHashSet::default()
        };
        let mut proven = Effects::PURE;
        let mut declared = Effects::PURE;
        for &unit in component {
            let (p, d) = local(unit);
            proven = union(proven, p);
            declared = union(declared, d);
            for edge in graph.edges_from(unit) {
                if *component == [edge.callee] || members.contains(&edge.callee) {
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
