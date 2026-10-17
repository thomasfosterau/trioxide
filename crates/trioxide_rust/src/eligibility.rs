//! The subset checker (PLAN §5): whether a function can be compiled to WASM, and if not, why.
//!
//! A function is eligible exactly when the backend can lower it (with every function it
//! calls) to RIR with JS-exact semantics, so the checker runs the lowering. Only facts that
//! pass [`Facts::is_usable`] count.

use oxc_allocator::Allocator;
use rustc_hash::FxHashMap;
use trioxide_analysis::Facts;
use trioxide_types::FunctionId;

use crate::{ExtraFacts, Reason, SourceModule, lower, world::World};

/// The checker's verdict on one function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eligibility {
    /// It can become (part of) an island.
    Eligible,
    /// It can't; the reasons have spans, possibly in the functions it calls.
    Ineligible(Vec<Reason>),
}

impl Eligibility {
    /// Whether the function is eligible.
    pub fn is_eligible(&self) -> bool {
        matches!(self, Self::Eligible)
    }
}

/// Checks `function` as an exported island function (its parameters and result cross the
/// boundary).
pub fn check(
    modules: &[SourceModule<'_>],
    facts: &Facts,
    extra: &dyn ExtraFacts,
    function: FunctionId,
) -> Eligibility {
    let allocators: Vec<Allocator> = modules.iter().map(|_| Allocator::default()).collect();
    let world = World::build(&allocators, modules);
    if !world.functions.contains_key(&function) {
        return Eligibility::Ineligible(vec![Reason {
            function,
            span: oxc_span::Span::default(),
            message: "isn't a function of the program".into(),
        }]);
    }
    let lowered = lower::lower_island(&world, facts, extra, "check", "check", &[function]);
    match lowered.failed.into_iter().next() {
        Some((_, reasons)) => Eligibility::Ineligible(reasons),
        None => Eligibility::Eligible,
    }
}

/// Clusters `eligible` functions into islands: the connected components of the call graph
/// restricted to them (PLAN D4, §5 "Clustering"). Components come out in the order of their
/// first function in `eligible`, each sorted the same way.
pub fn clusters(facts: &Facts, eligible: &[FunctionId]) -> Vec<Vec<FunctionId>> {
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let index: FxHashMap<FunctionId, usize> =
        eligible.iter().enumerate().map(|(i, &f)| (f, i)).collect();
    let mut parent: Vec<usize> = (0..eligible.len()).collect();
    for edge in facts.call_graph().edges() {
        if !facts.is_usable(edge.trust) {
            continue;
        }
        if let (Some(&a), Some(&b)) = (index.get(&edge.caller), index.get(&edge.callee)) {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                parent[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut groups: FxHashMap<usize, Vec<FunctionId>> = FxHashMap::default();
    let mut order = Vec::new();
    for (i, &function) in eligible.iter().enumerate() {
        let root = find(&mut parent, i);
        let group = groups.entry(root).or_default();
        if group.is_empty() {
            order.push(root);
        }
        group.push(function);
    }
    order
        .into_iter()
        .map(|root| groups.remove(&root).unwrap_or_default())
        .collect()
}
