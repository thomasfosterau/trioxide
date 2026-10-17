//! Runtime support for generated island crates.
//!
//! Every island crate that the Rust backend emits depends on `trioxide_rt`. S7 fills it with
//! (PLAN §6):
//!
//! - JS-exact numerics: `Math.round`, `sign`, `max`/`min` (NaN and -0), `trunc`, `fround`,
//!   `clz32`, `imul`, ToInt32/ToUint32 and bitwise operators, JS `%`;
//! - array helpers, including a checked index that yields `NaN` out of bounds;
//! - the ABI exports: a bump allocator (`__trioxide_alloc`, `__trioxide_reset`) and the memory
//!   layout for typed arrays and `number[]`.
//!
//! The crate is `no_std` and must build for `wasm32-unknown-unknown`.

#![no_std]

/// The `trioxide_wasm::ABI_VERSION` this runtime implements.
pub const ABI_VERSION: u32 = 0;
