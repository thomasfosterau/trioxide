//! Trioxide's whole-program analysis, stored as side tables.
//!
//! Analysis never mutates the AST. It produces [`Facts`]: tables keyed by
//! [`NodeRef`] / [`SymbolRef`] (a module id plus oxc's per-module `NodeId` / `SymbolId`), where
//! every entry carries a [`Trust`] level (PLAN D1). Optimisation passes and the partitioner
//! read facts and check trust before acting.
//!
//! S2 fills these tables: the linker, call graph, effect summaries, soundness refinement of
//! TS types, ranges, escape analysis and builtin models (PLAN §2 step 4). Until then
//! [`analyze`] returns empty tables, so every query answers `Unknown`.

mod facts;

pub use facts::{Effects, Fact, Facts, Trust};
use trioxide_common::Diagnostic;

/// Analysis settings derived from the user's options.
#[derive(Debug, Clone, Copy, Default)]
pub struct AnalysisOptions {
    /// Closed world (`app`): every caller of every function is visible (PLAN D2).
    pub closed_world: bool,
    /// Let `Declared` facts count as proof (`trustTypes`, PLAN D1). Passes consult
    /// [`Facts::is_usable`] rather than reading this directly.
    pub trust_types: bool,
}

/// The result of analysing a project.
#[derive(Debug, Default)]
pub struct Analysis {
    /// The fact tables.
    pub facts: Facts,
    /// Problems found while analysing (none today).
    pub diagnostics: Vec<Diagnostic>,
}

/// Analyses the whole program. Currently a no-op that returns empty facts.
///
/// S2 replaces this with the real pipeline; its inputs will grow to include the module graph,
/// each module's semantic model and the type oracle. The facade owns the call site, so the
/// signature can change without touching other streams.
pub fn analyze(options: AnalysisOptions) -> Analysis {
    Analysis {
        facts: Facts::new(options.trust_types),
        diagnostics: Vec::new(),
    }
}
