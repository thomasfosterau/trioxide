//! Trioxide's type lattice and the seam to TypeScript's checker.
//!
//! TS types come from [tsrs](https://github.com/oxc-project/tsrs), the oxc project's Rust port
//! of the TypeScript 7 checker (PLAN D3). This crate is the **only** one allowed to name tsrs
//! types: it converts every tsrs answer into the trioxide lattice in [`Type`], so tsrs bumps
//! stay local (PLAN §3, D10).
//!
//! The rest of trioxide talks to TypeScript through the [`TypeOracle`] trait. With the
//! default-on `tsrs` feature, [`TsrsOracle`] answers from an embedded tsrs program; without it,
//! [`NoopTypeOracle`] knows nothing and every TS type is unknown. [`default_oracle`] picks
//! whichever this build has.
//!
//! An oracle answers what TypeScript *declares*. It never decides trust: the analysis in
//! `trioxide_analysis` refines answers into `Proven` / `Declared` / `Unknown` facts (PLAN D1).

mod oracle;
#[cfg(feature = "tsrs")]
mod tsrs_adapter;
mod ty;

#[cfg(feature = "tsrs")]
pub use tsrs_adapter::{ProgramStats, RebuildPolicy, SpanStats, TsrsOracle};

pub use oracle::{
    CheckerQueries, NoopTypeOracle, OracleInput, SharedTypeQueries, SourceText, TsTypeFlags,
    TypeOracle,
};
pub use ty::{ClassId, FunctionId, Interval, NumberType, Shape, Signature, Type, TypedArrayKind};

/// Whether this build embeds the tsrs checker (the `tsrs` Cargo feature).
pub const HAS_TSRS: bool = cfg!(feature = "tsrs");

/// The best oracle this build has: [`TsrsOracle`] with the `tsrs` feature, else
/// [`NoopTypeOracle`].
pub fn default_oracle() -> Box<dyn TypeOracle> {
    #[cfg(feature = "tsrs")]
    {
        Box::new(TsrsOracle::default())
    }
    #[cfg(not(feature = "tsrs"))]
    {
        Box::new(NoopTypeOracle)
    }
}
