//! The cost model (PLAN §5): whether moving a cluster of eligible functions into WASM pays.
//!
//! *Benefit* is the compute the island takes over: weighted operations (arithmetic, element
//! accesses, `Math` calls, loop iterations) with every loop multiplied by its estimated trip
//! count, times the time WASM saves per operation. *Cost* is what crossing the boundary adds:
//! a fixed price per call into the island, the bytes copied through linear memory, and the
//! island's instantiation. Calls are weighted by the loops around their call sites, so a
//! kernel called per element from a JS loop pays per element.

use rustc_hash::{FxHashMap, FxHashSet};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use trioxide_analysis::Facts;
use trioxide_common::log::CostEstimate;
use trioxide_types::FunctionId;

use crate::metrics::Metrics;

/// The cost model's weights and thresholds (`costModel` in `trioxide.json`).
///
/// Operation weights are relative to one floating-point operation; `saving` converts them to
/// nanoseconds. The defaults come from `benchmarks/partition/` on V8 (Node 22).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CostModel {
    /// In `auto` mode an island is kept when `benefit − cost` exceeds this (nanoseconds).
    pub threshold: f64,
    /// The threshold in `wasm-preferred` mode.
    pub wasm_preferred_threshold: f64,
    /// Nanoseconds WASM saves per weighted operation.
    pub saving: f64,
    /// Weight of a floating-point arithmetic operation or comparison.
    pub arithmetic: f64,
    /// Multiplier for operations on proven integers and bitwise operators.
    pub integer: f64,
    /// Weight of a `Math.*` call or `**`.
    pub math: f64,
    /// Weight of an array element read or write.
    pub index: f64,
    /// Multiplier for typed array element accesses.
    pub typed_array: f64,
    /// Weight of one loop iteration's own control flow.
    pub iteration: f64,
    /// Trip count assumed for loops without a constant bound, and the element count assumed
    /// for arrays crossing the boundary.
    pub trips: f64,
    /// Nanoseconds per call into an island (the glue's checks and the arena reset).
    pub crossing: f64,
    /// Nanoseconds per byte copied into or out of linear memory.
    pub byte: f64,
    /// Multiplier for the bytes of a `number[]`, which is converted element by element.
    pub number_array: f64,
    /// Nanoseconds to instantiate one island.
    pub instantiation: f64,
}

impl Default for CostModel {
    fn default() -> Self {
        Self {
            threshold: 0.0,
            wasm_preferred_threshold: -50_000.0,
            saving: 0.35,
            arithmetic: 1.0,
            integer: 1.5,
            math: 4.0,
            index: 1.0,
            typed_array: 1.5,
            iteration: 1.0,
            trips: 256.0,
            crossing: 120.0,
            byte: 0.15,
            number_array: 4.0,
            instantiation: 20_000.0,
        }
    }
}

/// One scored cluster.
#[derive(Debug, Clone)]
pub(crate) struct Score {
    pub estimate: CostEstimate,
    /// The members JS calls into (or the host may call): the island's exports.
    pub entries: Vec<FunctionId>,
}

impl CostModel {
    /// Scores `cluster` against `threshold`.
    pub(crate) fn score(
        &self,
        cluster: &[FunctionId],
        facts: &Facts,
        metrics: &Metrics,
        threshold: f64,
    ) -> Score {
        let members: FxHashSet<FunctionId> = cluster.iter().copied().collect();
        let mut works = FxHashMap::default();
        let (mut calls, mut work, mut bytes) = (0.0, 0.0, 0.0);
        let mut entries = Vec::new();
        for &function in cluster {
            let mut freq = 0.0;
            let mut outside = false;
            for edge in facts.call_graph().callers(function) {
                if members.contains(&edge.caller) {
                    continue;
                }
                outside = true;
                freq += metrics.sites.get(&edge.site).copied().unwrap_or(1.0);
            }
            // Code trioxide doesn't see (the host calling an export) calls it at least once.
            let open = facts.function(function).is_none_or(|info| info.open);
            if open || !outside && facts.call_graph().callers(function).next().is_none() {
                freq += 1.0;
                outside = true;
            }
            if !outside {
                continue;
            }
            entries.push(function);
            calls += freq;
            work += freq * self.work(function, facts, metrics, &mut works, &mut Vec::new());
            bytes += freq * metrics.bytes(function, facts, self);
        }
        let benefit = work * self.saving;
        let crossings = calls * self.crossing;
        let marshalling = bytes * self.byte;
        let instantiation = self.instantiation;
        Score {
            estimate: CostEstimate {
                benefit,
                cost: crossings + marshalling + instantiation,
                threshold,
                crossings,
                marshalling,
                instantiation,
                calls,
                work,
                bytes,
            },
            entries,
        }
    }

    /// The weighted operations one call of `function` runs, its callees' included. A
    /// recursive call counts as a loop around the function's own work.
    fn work(
        &self,
        function: FunctionId,
        facts: &Facts,
        metrics: &Metrics,
        memo: &mut FxHashMap<FunctionId, f64>,
        stack: &mut Vec<FunctionId>,
    ) -> f64 {
        if let Some(&work) = memo.get(&function) {
            return work;
        }
        let Some(own) = metrics.functions.get(&function) else {
            return 0.0;
        };
        stack.push(function);
        let mut work = own.work;
        for &(site, freq) in &own.calls {
            let callees: Vec<FunctionId> = facts
                .call_graph()
                .callees(site)
                .map(|edge| edge.callee)
                .collect();
            if callees.is_empty() {
                continue;
            }
            let mut total = 0.0;
            for &callee in &callees {
                total += if stack.contains(&callee) {
                    let own = metrics.functions.get(&callee).map_or(0.0, |m| m.work);
                    own * self.trips
                } else {
                    self.work(callee, facts, metrics, memo, stack)
                };
            }
            #[expect(clippy::cast_precision_loss, reason = "callee counts are small")]
            let average = total / callees.len() as f64;
            work += freq * average;
        }
        stack.pop();
        memo.insert(function, work);
        work
    }
}
