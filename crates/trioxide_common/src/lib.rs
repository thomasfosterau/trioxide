//! Vocabulary shared by every trioxide crate.
//!
//! This crate sits at the bottom of the workspace so that the stream-owned crates (types,
//! analysis, JS passes, partitioner, WASM driver) can exchange ids, diagnostics and
//! decision-log records without depending on each other or on the facade.
//!
//! - [`ModuleId`], [`NodeRef`], [`SymbolRef`]: global identities across the module graph.
//! - [`Diagnostic`]: a serialisable, span-carrying error or warning.
//! - [`log`]: the decision log behind `trioxide explain` (PLAN §2 step 8).

mod diagnostic;
mod ids;
pub mod log;
mod serde_span;

pub use diagnostic::{Diagnostic, Label, Severity};
pub use ids::{ModuleId, NodeRef, SymbolRef};
pub use oxc_span::Span;
pub use oxc_syntax::{node::NodeId, symbol::SymbolId};
