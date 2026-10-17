//! Decides, per function, whether code stays JS or becomes part of a WASM island (PLAN §5).
//!
//! The full partitioner (S8) marks eligible functions, clusters them into islands over the
//! call graph, scores islands with the cost model, and honours `"use wasm"` / `"use js"`
//! directives and config overrides. For now [`Partitioner::partition`] keeps everything in JS.

use serde::{Deserialize, Serialize};
use trioxide_analysis::Facts;
use trioxide_common::ModuleId;
use trioxide_wasm::IslandId;

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
