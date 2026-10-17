//! Trioxide: a TypeScript-aware optimising compiler built on oxc.
//!
//! Per function, trioxide either emits optimised, readable JavaScript or transpiles it to Rust
//! compiled to WebAssembly, wired together with generated glue. See `docs/PLAN.md` for the
//! design.
//!
//! This crate is the facade every front end uses (the `trioxide` CLI, the napi binding and
//! through it the Vite plugin):
//!
//! - [`Options`]: configuration, read from `trioxide.json` / `trioxide.jsonc`.
//! - [`Project`]: the whole-program core with `build`, `update`, `transform` and `explain`.
//! - [`ModuleOutput`], [`BuildOutput`], [`Report`], [`Diagnostic`]: results.
//!
//! ```no_run
//! use trioxide::{Options, Project};
//!
//! let mut project = Project::new(".".into(), Options::default())?;
//! let out = project.transform("src/main.ts".as_ref())?;
//! print!("{}", out.code);
//! # Ok::<(), trioxide::Error>(())
//! ```
//!
//! Status: the pipeline is a passthrough (parse, strip TypeScript, print with sourcemaps).
//! Analysis, optimisation passes and WASM islands plug in behind the same API.

mod error;
mod options;
mod output;
mod pipeline;
mod project;
mod report;

pub use error::{Error, Result};
pub use options::{
    CONFIG_FILE_NAMES, DevOptions, GlueMode, Mode, Options, Override, OverrideTarget, WasmOptions,
    World,
};
pub use output::{BuildOutput, Invalidation, ModuleOutput, SourceMap};
pub use project::{Project, strip_types};
pub use report::{ExplainQuery, ModuleReport, Report};
pub use trioxide_common::{Diagnostic, Label, Severity, Span, log};
pub use trioxide_wasm::{IslandArtifact, IslandId, IslandRef, IslandSpec, WASM_URL_PLACEHOLDER};

/// Whether this build embeds the tsrs type checker (the `tsrs` Cargo feature).
pub const HAS_TSRS: bool = trioxide_types::HAS_TSRS;
