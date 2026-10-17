//! JavaScript's numeric conversions (ECMA-262 §7.1).

use crate::fpu;

const TWO_POW_31: f64 = 2_147_483_648.0;
const TWO_POW_32: f64 = 4_294_967_296.0;

/// `ToInt32`: what `x | 0` computes. NaN and ±Infinity become 0; everything else is truncated
/// and wrapped modulo 2^32 into the signed range.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "the fast path is in range and the slow path wraps on purpose"
)]
pub fn to_int32(x: f64) -> i32 {
    if x.abs() < TWO_POW_31 {
        x as i32
    } else {
        to_uint32(x) as i32
    }
}

/// `ToUint32`: what `x >>> 0` computes. NaN and ±Infinity become 0; everything else is
/// truncated and wrapped modulo 2^32.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "conversions wrap modulo 2^32 on purpose; every intermediate is exact"
)]
pub fn to_uint32(x: f64) -> u32 {
    if (0.0..TWO_POW_32).contains(&x) {
        x as u32
    } else if x.is_finite() {
        // `%` is exact, so the remainder lies in (-2^32, 2^32) and fits i64.
        (fpu::trunc(x) % TWO_POW_32) as i64 as u32
    } else {
        0
    }
}

/// `ToBoolean` for numbers: `false` for ±0 and NaN, `true` otherwise. This is how `if (x)`,
/// `!x`, `x && y` and `Boolean(x)` see a number.
pub fn to_boolean(x: f64) -> bool {
    x != 0.0 && !x.is_nan()
}

/// A boolean as a number, as `+b` or arithmetic on a boolean sees it.
pub fn bool_to_f64(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}
