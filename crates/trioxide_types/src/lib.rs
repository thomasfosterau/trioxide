//! Trioxide's type lattice and the seam to TypeScript's checker.
//!
//! TS types come from [tsrs](https://github.com/oxc-project/tsrs), the oxc project's Rust port
//! of the TypeScript 7 checker (PLAN D3). This crate is the **only** one allowed to name tsrs
//! types: it converts every tsrs answer into the trioxide lattice in [`Type`], so tsrs bumps
//! stay local (PLAN §3, D10).
//!
//! The rest of trioxide talks to TypeScript through the [`TypeOracle`] trait. The facade
//! currently uses [`NoopTypeOracle`], which knows nothing; S2 implements a tsrs-backed oracle
//! on top of [`tsrs_adapter`] (behind the default-on `tsrs` feature).
//!
//! An oracle answers what TypeScript *declares*. It never decides trust: the analysis in
//! `trioxide_analysis` refines answers into `Proven` / `Declared` / `Unknown` facts (PLAN D1).

mod oracle;
#[cfg(feature = "tsrs")]
pub mod tsrs_adapter;
mod ty;

pub use oracle::{NoopTypeOracle, OracleInput, SourceText, TypeOracle};
pub use ty::{ClassId, FunctionId, Interval, NumberType, Shape, Signature, Type, TypedArrayKind};

/// Whether this build embeds the tsrs checker (the `tsrs` Cargo feature).
pub const HAS_TSRS: bool = cfg!(feature = "tsrs");
