//! The WASM island contract and build driver.
//!
//! An *island* is a connected cluster of functions compiled together into one Rust crate and
//! one `.wasm` module (PLAN D4). This crate defines what an island looks like at the JS↔WASM
//! boundary ([`IslandSpec`]) and the seam that turns a generated Rust crate into a `.wasm`
//! file plus JS glue ([`IslandBuilder`]).
//!
//! - S6 (the Rust backend) *produces* `IslandSpec`s alongside the Rust source.
//! - S7 *implements* [`IslandBuilder`] (cargo driver, cache, `wasm-opt`, glue generator) and
//!   writes the full ABI up in `docs/ABI.md`, refining the types here additively.
//!
//! Until S7 lands, [`UnavailableIslandBuilder`] refuses every island, which makes callers fall
//! back to JS (PLAN D5).

use std::path::Path;

use serde::{Deserialize, Serialize};

pub mod toolchain;

/// The ABI version that [`IslandSpec`] describes. Bump on any incompatible change to how
/// values cross the boundary; generated glue checks it against the module's export.
pub const ABI_VERSION: u32 = 0;

/// The string a glue module uses in place of its `.wasm` file's URL, as in
/// `new URL("__TRIOXIDE_WASM_URL__", import.meta.url)`. Whoever emits the glue (the Vite
/// plugin, `trioxide build`) replaces it with the emitted asset's URL relative to the glue.
pub const WASM_URL_PLACEHOLDER: &str = "__TRIOXIDE_WASM_URL__";

/// A stable identifier for an island, unique within a build. It also names the island's
/// virtual glue module, `\0trioxide:island/<id>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IslandId(pub String);

impl IslandId {
    /// The specifier JS code uses to import this island's glue module (resolved by the Vite
    /// plugin or written to disk by `trioxide build`).
    pub fn glue_specifier(&self) -> String {
        format!("\0trioxide:island/{}", self.0)
    }
}

/// How a value crosses the JS↔WASM boundary (PLAN §5 eligibility, §6 "ABI and glue").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AbiType {
    /// A JS number passed as `f64`.
    F64,
    /// A JS number proven to be an int32, passed as `i32`.
    I32,
    /// A JS number proven to be a uint32, passed as `u32`.
    U32,
    /// A JS boolean, passed as `i32` 0/1.
    Bool,
    /// A typed array, copied into linear memory and passed as pointer + length.
    TypedArray {
        /// Element type.
        element: TypedArrayElement,
    },
    /// A `number[]`, converted to an `f64` buffer and passed as pointer + length.
    NumberArray,
    /// A plain-data struct, by index into [`IslandSpec::structs`].
    Struct {
        /// Index into [`IslandSpec::structs`].
        index: usize,
    },
}

/// The element type of a typed array crossing the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TypedArrayElement {
    /// `Int8Array`
    I8,
    /// `Uint8Array`
    U8,
    /// `Uint8ClampedArray`
    U8Clamped,
    /// `Int16Array`
    I16,
    /// `Uint16Array`
    U16,
    /// `Int32Array`
    I32,
    /// `Uint32Array`
    U32,
    /// `Float32Array`
    F32,
    /// `Float64Array`
    F64,
}

/// One parameter of an exported island function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AbiParam {
    /// The JS parameter name (for glue readability).
    pub name: String,
    /// How it crosses the boundary.
    pub ty: AbiType,
    /// For arrays: whether the island writes to it, so the glue must copy it back.
    #[serde(default)]
    pub mutated: bool,
}

/// A function the island exports to JS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandExport {
    /// The JS-visible name the glue exports, matching the original function.
    pub name: String,
    /// The symbol the `.wasm` module exports for it.
    pub wasm_name: String,
    /// Parameters in order.
    pub params: Vec<AbiParam>,
    /// The return type, or `None` for `void`.
    pub ret: Option<AbiType>,
}

/// A plain-data struct layout shared by the Rust side and the glue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructSpec {
    /// The class or type name it came from.
    pub name: String,
    /// Fields in layout order.
    pub fields: Vec<(String, AbiType)>,
}

/// The boundary contract of one island: what it exports and how values cross.
///
/// This is the interface between the Rust backend (S6), which generates the crate and the
/// spec, and the builder and glue generator (S7). Everything the glue needs must be here; the
/// glue never inspects the Rust source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandSpec {
    /// The island's id.
    pub id: IslandId,
    /// The [`ABI_VERSION`] the spec was produced for.
    pub abi_version: u32,
    /// Functions callable from JS.
    pub exports: Vec<IslandExport>,
    /// Struct layouts referenced by [`AbiType::Struct`].
    #[serde(default)]
    pub structs: Vec<StructSpec>,
}

/// A built island, ready to be emitted next to the JS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IslandArtifact {
    /// The spec it was built from.
    pub spec: IslandSpec,
    /// The compiled (and possibly `wasm-opt`-ed) module.
    pub wasm: Vec<u8>,
    /// The ESM glue module's source. It refers to the `.wasm` by [`WASM_URL_PLACEHOLDER`],
    /// which the emitter replaces.
    pub glue: String,
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
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The Rust toolchain or the wasm32 target is missing. Callers fall back to JS with one
    /// warning, unless `--require-wasm` is set (PLAN §6 "Build driver").
    #[error("Rust toolchain unavailable: {0}")]
    ToolchainUnavailable(String),
    /// `cargo build` failed; carries its stderr.
    #[error("cargo build failed for island {island}:\n{stderr}")]
    Cargo {
        /// The island that failed.
        island: String,
        /// cargo's output.
        stderr: String,
    },
    /// Reading or writing the cache or crate directory failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Turns a generated island crate into a [`IslandArtifact`].
///
/// Implementations shell out to `cargo build --target wasm32-unknown-unknown --release`, cache
/// by content hash, run `wasm-opt` if available, and generate the glue (PLAN §6). They must be
/// safe to call for several islands in parallel.
pub trait IslandBuilder: Send + Sync {
    /// Builds the crate at `crate_dir` (containing `Cargo.toml` and `src/lib.rs`) whose
    /// boundary is described by `spec`.
    fn build(&self, spec: &IslandSpec, crate_dir: &Path) -> Result<IslandArtifact, BuildError>;
}

/// The builder used until S7 lands: it never builds anything, so every island falls back to
/// JS.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableIslandBuilder;

impl IslandBuilder for UnavailableIslandBuilder {
    fn build(&self, spec: &IslandSpec, _crate_dir: &Path) -> Result<IslandArtifact, BuildError> {
        Err(BuildError::ToolchainUnavailable(format!(
            "island builds are not implemented yet (island {})",
            spec.id.0
        )))
    }
}
