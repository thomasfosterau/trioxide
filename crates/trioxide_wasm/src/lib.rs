//! The WASM island contract, build driver and JS glue.
//!
//! An *island* is a connected cluster of functions compiled together into one Rust crate and
//! one `.wasm` module (PLAN D4). This crate defines what an island looks like at the JS↔WASM
//! boundary ([`IslandSpec`], specified in `docs/ABI.md`), builds island crates into `.wasm`
//! files ([`CargoIslandBuilder`]), and generates the JS glue that loads and calls them
//! ([`generate_glue`]).
//!
//! - The Rust backend (S6) *produces* an [`IslandSource`] per island: the spec plus the
//!   crate's Rust files, in the shape `docs/ABI.md` prescribes.
//! - The facade hands those to an [`IslandBuilder`] and puts the resulting
//!   [`IslandArtifact`]s (wasm bytes plus glue) in its `BuildOutput`.
//! - [`toolchain`] finds cargo, rustc, the wasm32 target and `wasm-opt`, for the builder and
//!   for `trioxide check-toolchain`.
//!
//! When the toolchain is missing, builders return [`BuildError::ToolchainUnavailable`] and
//! callers fall back to the JS versions of the island's functions (PLAN D5).

mod builder;
mod glue;
mod spec;
pub mod toolchain;

use std::path::Path;

use serde::{Deserialize, Serialize};

pub use builder::{
    BuilderOptions, CargoIslandBuilder, IslandSource, RT_LIBM_REQUIREMENT, RT_VERSION,
};
pub use glue::{GlueMode, GlueOptions, WasmUrl, generate_glue};
pub use spec::{
    ABI_VERSION, AbiParam, AbiType, FieldLayout, IslandExport, IslandId, IslandSpec, SpecError,
    StructField, StructLayout, StructSpec, TypedArrayElement, WasmType,
};

/// A built island, ready to be emitted next to the JS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IslandArtifact {
    /// The spec it was built from.
    pub spec: IslandSpec,
    /// The compiled (and possibly `wasm-opt`-ed) module.
    pub wasm: Vec<u8>,
    /// The ESM glue module's source. By default it loads the module from
    /// [`wasm_file_name`](Self::wasm_file_name) next to itself
    /// ([`WasmUrl::Sibling`]).
    pub glue: String,
    /// Non-fatal problems, e.g. `wasm-opt` failing (the unoptimised module is used).
    pub warnings: Vec<String>,
}

impl IslandArtifact {
    /// `<id>.wasm`, the file name the default glue loads.
    pub fn wasm_file_name(&self) -> String {
        self.spec.wasm_file_name()
    }

    /// `<id>.js`, the conventional file name for the glue.
    pub fn glue_file_name(&self) -> String {
        format!("{}.js", self.spec.id.0)
    }
}

/// A reference from a JS module to an island it imports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandRef {
    /// Which island.
    pub id: IslandId,
    /// The specifier the module's code imports it by ([`IslandId::glue_specifier`]).
    pub specifier: String,
}

/// Why an island could not be built.
#[derive(Debug, Clone, thiserror::Error)]
pub enum BuildError {
    /// The Rust toolchain or the wasm32 target is missing. Callers fall back to JS with one
    /// warning, unless `--require-wasm` is set (PLAN §6 "Build driver").
    #[error("Rust toolchain unavailable: {0}")]
    ToolchainUnavailable(String),
    /// The island's spec or file list is malformed (a bug in whatever produced it).
    #[error(transparent)]
    InvalidSpec(#[from] SpecError),
    /// `cargo build` failed; carries its stderr.
    #[error("cargo build failed for island {island}:\n{stderr}")]
    Cargo {
        /// The island that failed.
        island: String,
        /// cargo's output.
        stderr: String,
    },
    /// Reading or writing the cache or an island directory failed.
    #[error("{path}: {message}")]
    Io {
        /// The file or directory.
        path: String,
        /// The error.
        message: String,
    },
}

impl BuildError {
    pub(crate) fn io(path: &Path, error: &std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            message: error.to_string(),
        }
    }
}

/// Turns island sources into [`IslandArtifact`]s.
///
/// Implementations must be safe to call from several threads.
pub trait IslandBuilder: Send + Sync {
    /// Builds one island.
    fn build(&self, island: &IslandSource) -> Result<IslandArtifact, BuildError>;

    /// Builds several islands, returning one result per island in order. Implementations
    /// should build them in parallel.
    fn build_all(&self, islands: &[IslandSource]) -> Vec<Result<IslandArtifact, BuildError>> {
        islands.iter().map(|island| self.build(island)).collect()
    }
}

/// A builder that never builds anything, so every island falls back to JS.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableIslandBuilder;

impl IslandBuilder for UnavailableIslandBuilder {
    fn build(&self, island: &IslandSource) -> Result<IslandArtifact, BuildError> {
        Err(BuildError::ToolchainUnavailable(format!(
            "island builds are disabled (island {})",
            island.spec.id
        )))
    }
}
