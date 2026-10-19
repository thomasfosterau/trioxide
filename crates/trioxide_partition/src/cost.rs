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
use trioxide_analysis::{CallEdge, Facts};
use trioxide_common::log::CostEstimate;
use trioxide_types::FunctionId;

use crate::metrics::{Metrics, Ops, saturate};

/// The cost model's weights and thresholds (`costModel` in `trioxide.json`).
///
/// Operation weights are the nanoseconds WASM saves per operation compared with V8's
/// optimised JS (negative where the island is slower); the cost terms are nanoseconds too.
/// The defaults are fitted to `benchmarks/partition/` (Node 22, V8 12.4) for the code the
/// Rust backend generates today.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CostModel {
    /// In `auto` mode an island is kept when `benefit − cost` exceeds this (nanoseconds).
    pub threshold: f64,
    /// The threshold in `wasm-preferred` mode.
    pub wasm_preferred_threshold: f64,
    /// Nanoseconds saved per floating-point arithmetic operation or comparison.
    pub arithmetic: f64,
    /// Nanoseconds saved per operation on values the island holds as int32s (arithmetic,
    /// comparisons, `++`, `--`, bitwise operators).
    pub integer: f64,
    /// Nanoseconds saved per float operand of a bitwise operator or `Math.imul` (a
    /// `ToInt32` in WASM).
    pub conversion: f64,
    /// Nanoseconds saved per `(a + b) | 0`-style wrapped operation (V8 leaves its int32 fast
    /// path when the sum overflows; WASM wraps natively).
    pub wrapping: f64,
    /// Nanoseconds saved per `Math.*` call or `**`.
    pub math: f64,
    /// Nanoseconds saved per `%` on values not proven integers (a call to `fmod` in WASM).
    pub remainder: f64,
    /// Nanoseconds saved per array element read or write whose index the island computes as
    /// an int32.
    pub index: f64,
    /// Nanoseconds saved per other element read or write (the index is a float, converted
    /// and checked on every access).
    pub checked_index: f64,
    /// Nanoseconds saved per loop iteration (its own control flow).
    pub iteration: f64,
    /// Trip count assumed for loops without a known bound, and the element count assumed
    /// for arrays of unknown length crossing the boundary.
    pub trips: f64,
    /// Nanoseconds per call into an island (the glue's checks and the arena reset).
    pub crossing: f64,
    /// Nanoseconds per array or object argument (or result) per call: allocating it in the
    /// arena, the glue's aliasing and plain-view checks, the views it creates.
    pub aggregate: f64,
    /// Nanoseconds per byte copied into or out of linear memory.
    pub byte: f64,
    /// Multiplier for the bytes of a `number[]`, which is converted element by element.
    pub number_array: f64,
    /// Nanoseconds to instantiate one island.
    pub instantiation: f64,
}

impl Default for CostModel {
    /// Fitted to `benchmarks/partition/` (see its README; refitted in S9 once the analysis
    /// proved loop counters int32): wrapped `(a + b) | 0` arithmetic and int32-indexed
    /// element accesses are faster in today's islands than in V8's optimised JS; other int32
    /// and floating-point arithmetic is about a tie; `ToInt32` of floats, float `%`,
    /// float-indexed accesses and every crossing cost more in WASM. The fitted values are
    /// kept to three significant figures: `integer` and `iteration` are collinear, and
    /// rounding either one further changes picks.
    /// Instantiation is charged below its measured 0.65 ms, as `ready()` glue compiles off
    /// the main thread and runs the JS versions meanwhile.
    fn default() -> Self {
        Self {
            threshold: 0.0,
            wasm_preferred_threshold: -1_000_000.0,
            arithmetic: -0.0914,
            integer: -0.116,
            conversion: -0.358,
            wrapping: 0.704,
            math: -0.0142,
            remainder: -1.88,
            index: -0.117,
            checked_index: -7.7,
            iteration: 1.23,
            trips: 256.0,
            crossing: 12.9,
            aggregate: 52.9,
            byte: 0.12,
            number_array: 12.0,
            instantiation: 300_000.0,
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
    /// Scores `cluster` against `threshold`. Only the calls `reaches` accepts count: the
    /// ones the facade will point at the island (direct calls of exported bindings from
    /// modules that import them, or from their own module). The host calling an entry
    /// module's export runs its JS version, so it doesn't count.
    pub(crate) fn score(
        &self,
        cluster: &[FunctionId],
        facts: &Facts,
        metrics: &Metrics,
        threshold: f64,
        reaches: &dyn Fn(&CallEdge) -> bool,
    ) -> Score {
        let members: FxHashSet<FunctionId> = cluster.iter().copied().collect();
        let mut works = FxHashMap::default();
        let mut recursive = false;
        let (mut calls, mut bytes, mut aggregates) = (0.0, 0.0, 0.0);
        let mut ops = Ops::default();
        let mut entries = Vec::new();
        for &function in cluster {
            let freq: f64 = facts
                .call_graph()
                .callers(function)
                .filter(|edge| !members.contains(&edge.caller) && reaches(edge))
                .map(|edge| {
                    metrics.sites.get(&edge.site).copied().unwrap_or(1.0)
                        * metrics.runs(edge.caller)
                })
                .sum();
            if freq == 0.0 {
                continue;
            }
            entries.push(function);
            calls = saturate(calls + freq);
            ops.add_scaled(
                &self.work(
                    function,
                    facts,
                    metrics,
                    &mut works,
                    &mut Vec::new(),
                    &mut recursive,
                ),
                freq,
            );
            bytes = saturate(bytes + freq * metrics.bytes(function, facts, self));
            aggregates = saturate(aggregates + freq * metrics.aggregates(function, facts));
        }
        let benefit = ops.saving(self);
        let work = ops.total();
        let crossings = calls * self.crossing;
        let marshalling = bytes * self.byte + aggregates * self.aggregate;
        // Lowering makes one island per module of the exports.
        let mut modules: Vec<_> = entries.iter().map(|f| f.module).collect();
        modules.sort_unstable();
        modules.dedup();
        #[expect(clippy::cast_precision_loss, reason = "module counts are small")]
        let instantiation = self.instantiation * modules.len() as f64;
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
                recursive,
            },
            entries,
        }
    }

    /// The operations one call of `function` runs, its callees' included. A recursive call
    /// counts as a loop around the function's own work (and sets `recursive`).
    fn work(
        &self,
        function: FunctionId,
        facts: &Facts,
        metrics: &Metrics,
        memo: &mut FxHashMap<FunctionId, Ops>,
        stack: &mut Vec<FunctionId>,
        recursive: &mut bool,
    ) -> Ops {
        if let Some(&ops) = memo.get(&function) {
            return ops;
        }
        let Some(own) = metrics.functions.get(&function) else {
            return Ops::default();
        };
        stack.push(function);
        let mut ops = own.ops;
        for &(site, freq) in &own.calls {
            let callees: Vec<FunctionId> = facts
                .call_graph()
                .callees(site)
                .map(|edge| edge.callee)
                .collect();
            #[expect(clippy::cast_precision_loss, reason = "callee counts are small")]
            let share = freq / callees.len().max(1) as f64;
            for &callee in &callees {
                if stack.contains(&callee) {
                    *recursive = true;
                    if let Some(callee) = metrics.functions.get(&callee) {
                        ops.add_scaled(&callee.ops, share * self.trips);
                    }
                } else {
                    let callee = self.work(callee, facts, metrics, memo, stack, recursive);
                    ops.add_scaled(&callee, share);
                }
            }
        }
        stack.pop();
        memo.insert(function, ops);
        ops
    }
}
