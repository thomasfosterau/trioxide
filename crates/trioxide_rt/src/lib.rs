//! Runtime support for generated island crates.
//!
//! Every island crate depends on `trioxide_rt` (see `docs/ABI.md` for the exact shape of an
//! island). It provides:
//!
//! - [`math`], [`conv`], [`ops`]: JavaScript-exact numerics. `Math.*` with JS's NaN and -0
//!   rules, ToInt32/ToUint32 and the bitwise operators, JS `%` and integer division.
//! - [`array`]: typed-array element conversions ([`array::Element`]), a checked index that
//!   yields `NaN` out of bounds like `undefined` would in arithmetic, and unchecked variants.
//! - [`abi`]: the boundary. A bump arena exported as `__trioxide_alloc` / `__trioxide_reset`,
//!   argument decoding and return helpers.
//!
//! The crate is `no_std` and has no allocator. On wasm32 the island builder enables the `std`
//! feature, used only for the hardware float instructions (see the feature's documentation in
//! `Cargo.toml`). Generated code never panics: compiler bugs surface as a trap, which the glue
//! reports.
//!
//! Generated code imports the [`prelude`] and calls helpers by module path, e.g.
//! `math::round(x)`, `ops::shl(a, b)`, `array::get(xs, i)`.

#![cfg_attr(not(test), no_std)]
#![expect(
    clippy::float_cmp,
    clippy::neg_cmp_op_on_partial_ord,
    reason = "JavaScript semantics are exact IEEE comparisons, NaN included"
)]

#[cfg(feature = "std")]
extern crate std;

pub mod abi;
pub mod array;
pub mod conv;
mod fpu;
pub mod math;
pub mod ops;

/// What generated island code imports: `use trioxide_rt::prelude::*;`.
pub mod prelude {
    pub use crate::{
        abi,
        array::{self, Element, U8Clamped},
        conv, math, ops,
    };
}

/// The ABI version this runtime implements. It must equal `trioxide_wasm::ABI_VERSION`; the
/// glue checks it against the module's `__trioxide_abi_version` export.
pub const ABI_VERSION: u32 = 1;

/// This crate's version, part of the island build cache key.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(all(target_arch = "wasm32", not(feature = "std"), not(test)))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}
