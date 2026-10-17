//! The project layer: which files make up a program, and what changed.
//!
//! - [`ModuleGraph`] discovers modules from entry points by parsing their imports and resolving
//!   them with `oxc_resolver`, honouring `tsconfig.json` `paths` (PLAN §2 step 1, D8).
//! - [`CompilerSettings`] is the subset of `tsconfig.json` trioxide reads (`target`, `jsx`,
//!   class-field and decorator semantics, `verbatimModuleSyntax`) (PLAN D11).
//! - [`ContentHash`] and [`ContentCache`] let later stages skip unchanged modules.
//!
//! Sources normally come from disk, but any path can be overridden in memory (Vite hands the
//! plugin code that may differ from disk), via [`ModuleGraph::set_source`].

mod cache;
mod graph;
mod tsconfig;

pub use cache::{ContentCache, ContentHash};
pub use graph::{
    GraphOptions, Import, ImportKind, ImportTarget, Module, ModuleGraph, is_module_path,
};
pub use tsconfig::{CompilerSettings, JsxMode, find_tsconfig};

/// Errors from the project layer that abort the operation (per-module problems are
/// [`Diagnostic`](trioxide_common::Diagnostic)s instead).
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    /// An entry point could not be read.
    #[error("cannot read entry {path}: {source}")]
    Entry {
        /// The entry's path.
        path: std::path::PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The tsconfig could not be loaded.
    #[error("cannot load tsconfig {path}: {message}")]
    Tsconfig {
        /// The tsconfig's path.
        path: std::path::PathBuf,
        /// What went wrong.
        message: String,
    },
}
