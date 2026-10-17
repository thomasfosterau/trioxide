//! Decides, per function, whether code stays JS or becomes part of a WASM island (PLAN §5).
//!
//! The full partitioner (S8) marks eligible functions, clusters them into islands over the
//! call graph, scores islands with the cost model, and honours `"use wasm"` / `"use js"`
//! directives and config overrides. For now only directives create islands
//! ([`Partitioner::partition_sources`], via the Rust backend's
//! [`forced_islands`](trioxide_rust::forced_islands)); [`Partitioner::partition`], which sees
//! no sources, keeps everything in JS.

use serde::{Deserialize, Serialize};
use trioxide_analysis::Facts;
use trioxide_common::{Diagnostic, ModuleId, log::FunctionDecision};
pub use trioxide_rust::{ExportBinding, SourceModule};
use trioxide_wasm::{IslandId, IslandSource};

/// How aggressively to move code to WASM (`Options::mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PartitionMode {
    /// Never create islands; only the JS passes run.
    JsOnly,
    /// Create islands the cost model says are profitable.
    #[default]
    Auto,
    /// Create an island for every eligible cluster unless the cost model strongly objects.
    WasmPreferred,
}

/// Inputs to partitioning that come from options rather than analysis.
#[derive(Debug, Clone, Copy, Default)]
pub struct PartitionOptions {
    /// The partition mode.
    pub mode: PartitionMode,
    /// Whether WASM output is enabled at all (`--no-wasm` turns it off).
    pub wasm: bool,
}

/// A planned island: the functions that compile together.
#[derive(Debug, Clone, PartialEq)]
pub struct IslandPlan {
    /// The island's id.
    pub id: IslandId,
    /// Its member functions.
    pub functions: Vec<trioxide_types::FunctionId>,
    /// The modules whose code calls into it (they import its glue).
    pub importers: Vec<ModuleId>,
    /// The module whose functions it exports, for islands forced by directives.
    pub module: Option<ModuleId>,
    /// The functions it exports to JS, with their names.
    pub exports: Vec<(trioxide_types::FunctionId, String)>,
    /// The exports JS calls may be pointed at (their bindings are never reassigned).
    pub bindings: Vec<ExportBinding>,
    /// The generated crate, once the Rust backend has lowered it.
    pub source: Option<IslandSource>,
    /// Some export may be called with overlapping arguments it writes to, so the glue needs
    /// the JS versions as a `fallback` (`docs/ABI.md`, "Aliasing").
    pub needs_fallback: bool,
}

/// The partitioner's output.
///
/// Per-function verdicts (with reasons) go into each module's
/// [`ModuleLog`](trioxide_common::log::ModuleLog) for `explain`; this holds what later stages
/// act on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Partition {
    /// Islands to build. Every function not in an island stays JS.
    pub islands: Vec<IslandPlan>,
    /// Per-function verdicts, for each module's decision log.
    pub decisions: Vec<(ModuleId, FunctionDecision)>,
    /// Problems with directives (a forced function that can't be compiled is an error).
    pub diagnostics: Vec<Diagnostic>,
}

impl Partition {
    /// Whether everything stays JS.
    pub fn is_all_js(&self) -> bool {
        self.islands.is_empty()
    }
}

/// Splits a program into JS and WASM islands.
#[derive(Debug, Clone, Default)]
pub struct Partitioner {
    options: PartitionOptions,
}

impl Partitioner {
    /// A partitioner with the given options.
    pub fn new(options: PartitionOptions) -> Self {
        Self { options }
    }

    /// The options in effect.
    pub fn options(&self) -> PartitionOptions {
        self.options
    }

    /// Partitions the program described by `facts`. Currently always "all JS".
    pub fn partition(&self, facts: &Facts) -> Partition {
        let _ = facts;
        Partition::default()
    }

    /// Partitions the program in `modules`: one island per module with functions that a
    /// `"use wasm"` directive or `/** @trioxide wasm */` tag forces into WASM (directives
    /// override the mode), lowered by the Rust backend. Nothing when WASM is off.
    pub fn partition_sources(&self, facts: &Facts, modules: &[SourceModule<'_>]) -> Partition {
        if !self.options.wasm {
            return Partition::default();
        }
        let forced = trioxide_rust::forced_islands(modules, facts, &trioxide_rust::NoExtraFacts);
        let islands = forced
            .islands
            .into_iter()
            .map(|island| {
                let mut importers = vec![island.module];
                for edge in facts.call_graph().edges() {
                    let exported = island.exports.iter().any(|(f, _)| *f == edge.callee);
                    if exported
                        && !island.members.contains(&edge.caller)
                        && !importers.contains(&edge.caller.module)
                    {
                        importers.push(edge.caller.module);
                    }
                }
                IslandPlan {
                    id: island.source.spec.id.clone(),
                    functions: island.members,
                    importers,
                    module: Some(island.module),
                    exports: island.exports,
                    bindings: island.bindings,
                    source: Some(island.source),
                    needs_fallback: island.needs_fallback,
                }
            })
            .collect();
        Partition {
            islands,
            decisions: forced.decisions,
            diagnostics: forced.diagnostics,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_stays_js_for_now() {
        let partitioner = Partitioner::new(PartitionOptions {
            mode: PartitionMode::WasmPreferred,
            wasm: true,
        });
        assert!(partitioner.partition(&Facts::new(false)).is_all_js());
    }
}
