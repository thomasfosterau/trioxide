//! Trioxide's JavaScript backend.
//!
//! - [`PassManager`] runs [`Pass`]es (the optimisations of PLAN §4) over one module's AST,
//!   keeping `oxc_semantic` data in sync between pass groups.
//! - [`emit`] strips TypeScript with `oxc_transformer` (honouring `tsconfig` `target`, `jsx`,
//!   class-field and decorator semantics) and prints the result with `oxc_codegen`, sourcemap
//!   included.
//!
//! - [`passes`] holds the optimisation passes and the default pipeline.
//!
//! S4 and S5 add passes here; the facade only depends on [`PassManager`] and [`emit`].

mod emit;
mod pass;
pub mod passes;

pub use emit::{EmitOptions, EmitOutput, emit, emit_with, transform_options};
pub use pass::{
    FactSource, Pass, PassContext, PassGroup, PassManager, PassOptions, rebuild_scoping,
};
