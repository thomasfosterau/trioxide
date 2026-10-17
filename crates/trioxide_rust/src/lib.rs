//! The Rust backend: turns eligible functions into Rust source for a WASM island.
//!
//! S6 builds this crate (PLAN §6):
//!
//! - a **subset checker** that decides whether a function body is in the compilable subset
//!   (PLAN §5 "Eligibility") and explains why not;
//! - **RIR**, a small typed tree IR (functions, structs, `f64`/`i32`/`u32`/`bool` locals, slices
//!   into linear memory, loops, calls), lowered from the oxc AST plus analysis facts;
//! - an **emitter** that prints RIR as Rust (via `prettyplease`) into an island crate depending
//!   on `trioxide_rt`, together with the island's
//!   [`IslandSpec`](trioxide_wasm::IslandSpec).
//!
//! The RIR stays backend-agnostic so a direct `wasm-encoder` backend remains possible (D6).
//! Nothing is implemented yet.

pub use trioxide_wasm::IslandSpec;
