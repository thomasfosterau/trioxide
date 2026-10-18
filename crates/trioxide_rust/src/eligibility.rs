//! The subset checker (PLAN §5): whether a function can be compiled to WASM, and if not, why.
//!
//! A function is eligible exactly when the backend can lower it (with every function it
//! calls) to RIR with JS-exact semantics, so the checker runs the lowering. Only facts that
//! pass [`Facts::is_usable`] count.

use oxc_allocator::Allocator;
use rustc_hash::FxHashMap;
use trioxide_analysis::Facts;
use trioxide_common::{ModuleId, Span};
use trioxide_types::FunctionId;

use crate::{
    Directive, ExtraFacts, Placement, Reason, SourceModule, automatic_root, lower, never_called,
    world::World,
};

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
            span: Span::default(),
            message: "isn't a function of the program".into(),
        }]);
    }
    let lowered = lower::lower_island(&world, facts, extra, "check", "check", &[function]);
    match lowered.failed.into_iter().next() {
        Some((_, reasons)) => Eligibility::Ineligible(reasons),
        None => Eligibility::Eligible,
    }
}

/// A function the partitioner may move into WASM itself: a top-level function declaration
/// whose name can be an island export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The function.
    pub function: FunctionId,
    /// Its name.
    pub name: String,
    /// Its span.
    pub span: Span,
    /// The directive or config override that places it, if any: then that decides, not the
    /// cost model.
    pub directive: Option<Directive>,
    /// The analysis proves nothing calls it (it is unreachable, or a parameter never
    /// receives a value).
    pub never_called: bool,
    /// Its module exports it and never reassigns the binding, so calls from other modules
    /// (and, with `ready()` glue, from its own) can be pointed at an island.
    pub exported: bool,
    /// Whether it can be an island export, checked unless `directive` or `never_called`
    /// settles it.
    pub eligibility: Option<Eligibility>,
}

/// Every candidate of the program (see [`Candidate`]), checked as in [`check`] but with one
/// parse of the program. `overrides` places whole modules as config overrides do.
pub fn candidates(
    modules: &[SourceModule<'_>],
    facts: &Facts,
    extra: &dyn ExtraFacts,
    overrides: &[(ModuleId, Placement)],
) -> Vec<Candidate> {
    let allocators: Vec<Allocator> = modules.iter().map(|_| Allocator::default()).collect();
    let mut world = World::build(&allocators, modules);
    world.set_overrides(overrides);
    let mut functions: Vec<_> = world
        .functions
        .values()
        .filter(|info| automatic_root(info))
        .collect();
    functions.sort_by_key(|info| (info.id.module, info.func.span.start));
    let mut out = Vec::new();
    for module in &world.modules {
        let first = out.len();
        let mut roots = Vec::new();
        for info in functions.iter().filter(|f| f.id.module == module.id) {
            let directive = world.directive(info.id);
            let never = never_called(module, facts, info.func);
            let exported = info
                .func
                .id
                .as_ref()
                .and_then(|id| id.symbol_id.get())
                .is_some_and(|symbol| {
                    let (names, reassigned) = module.exported_names(symbol);
                    !names.is_empty() && !reassigned
                });
            if directive.is_none() && !never {
                roots.push(info.id);
            }
            out.push(Candidate {
                function: info.id,
                name: info.name.clone().unwrap_or_default(),
                span: info.func.span,
                directive,
                never_called: never,
                exported,
                eligibility: None,
            });
        }
        if roots.is_empty() {
            continue;
        }
        let lowered = lower::lower_island(&world, facts, extra, "check", "check", &roots);
        let mut failed: FxHashMap<FunctionId, Vec<Reason>> = lowered.failed.into_iter().collect();
        let unsettled = failed
            .values()
            .flatten()
            .any(|r| r.message.contains("didn't settle"));
        if roots.len() > 1 && unsettled {
            // Together they may not settle; alone some might.
            failed.clear();
            for &root in &roots {
                let alone = lower::lower_island(&world, facts, extra, "check", "check", &[root]);
                failed.extend(alone.failed);
            }
        }
        for candidate in &mut out[first..] {
            if roots.contains(&candidate.function) {
                candidate.eligibility = Some(
                    failed
                        .remove(&candidate.function)
                        .map_or(Eligibility::Eligible, Eligibility::Ineligible),
                );
            }
        }
    }
    out
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
