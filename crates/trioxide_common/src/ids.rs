use std::fmt;

use oxc_syntax::{node::NodeId, symbol::SymbolId};
use serde::{Deserialize, Serialize};

/// Identifies one module in a project's module graph.
///
/// Ids are dense indices assigned in discovery order by `trioxide_project::ModuleGraph` and
/// are stable for the life of a `Project`: updating a module's source keeps its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModuleId(u32);

impl ModuleId {
    /// Wraps a raw index.
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    /// The raw index, usable to index per-module vectors.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "m{}", self.0)
    }
}

/// An AST node in a specific module: oxc's per-module [`NodeId`] made global.
///
/// `NodeId`s come from `oxc_semantic` and are only meaningful for the AST they were built
/// from. Any pass that rewrites a module's AST invalidates its `NodeRef`s; the pass manager
/// rebuilds semantic data and the affected analysis tables between pass groups (PLAN §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeRef {
    /// The module the node belongs to.
    pub module: ModuleId,
    /// The node within that module's semantic model.
    pub node: NodeId,
}

/// A symbol (binding) in a specific module: oxc's per-module [`SymbolId`] made global.
///
/// The linker in `trioxide_analysis` resolves imports to the `SymbolRef` of the exporting
/// module's binding, so two `SymbolRef`s are equal exactly when they name the same binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolRef {
    /// The module that declares the symbol.
    pub module: ModuleId,
    /// The symbol within that module's scoping.
    pub symbol: SymbolId,
}
