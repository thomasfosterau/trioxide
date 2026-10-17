//! JavaScript operators whose Rust spelling differs: bitwise operators, `%` on integers and
//! integer division.
//!
//! Operators that need no helper, because Rust's `f64` semantics already are JavaScript's:
//!
//! - `+ - * /` on numbers: IEEE 754 double arithmetic, round to nearest even.
//! - `%` on numbers: Rust's `f64 % f64` is C's `fmod`, which is exactly JavaScript's `%`
//!   (sign of the dividend, NaN for `x % 0` and `±Infinity % y`, `x % ±Infinity = x`).
//!   [`rem`] exists so generated code can spell it uniformly.
//! - `< <= > >= == !=` (and `===`, `!==` on numbers): IEEE comparisons, so every comparison
//!   with NaN is false except `!=`, and `-0 == 0`. Never rewrite `!(a < b)` as `a >= b`.
//! - unary `-`: flips the sign, so `-0` exists. An `i32` local can't hold `-0`: lower
//!   `-x` to an integer only where the result is used as an int32 (`(-x) | 0`) or `x` is
//!   proven non-zero.
//!
//! Integer (`i32`) arithmetic is only used where range analysis proves no overflow, since
//! JavaScript would produce a non-int32 number instead of wrapping; `wrapping_*` spells the
//! cases where wrapping is the JavaScript semantics (`Math.imul`, `(a + b) | 0`).

use crate::conv::{to_int32, to_uint32};

/// `a % b` on numbers.
pub fn rem(a: f64, b: f64) -> f64 {
    a % b
}

/// `a % b` on int32 operands. The result is a number, not an int32: `x % 0` is NaN and a
/// zero remainder of a negative dividend is -0 (`-4 % 2` is `-0`).
pub fn rem_i32(a: i32, b: i32) -> f64 {
    if b == 0 {
        return f64::NAN;
    }
    let r = a.wrapping_rem(b);
    if r == 0 && a < 0 { -0.0 } else { f64::from(r) }
}

/// `(a / b) | 0` on int32 operands: division truncating toward zero, 0 when `b` is 0, and
/// `-2147483648 / -1` wraps to -2147483648.
pub fn div_trunc_i32(a: i32, b: i32) -> i32 {
    if b == 0 { 0 } else { a.wrapping_div(b) }
}

/// `a & b`.
pub fn bit_and(a: f64, b: f64) -> i32 {
    to_int32(a) & to_int32(b)
}

/// `a | b`.
pub fn bit_or(a: f64, b: f64) -> i32 {
    to_int32(a) | to_int32(b)
}

/// `a ^ b`.
pub fn bit_xor(a: f64, b: f64) -> i32 {
    to_int32(a) ^ to_int32(b)
}

/// `~a`.
pub fn bit_not(a: f64) -> i32 {
    !to_int32(a)
}

/// `a << b`: the shift count is taken modulo 32.
pub fn shl(a: f64, b: f64) -> i32 {
    to_int32(a).wrapping_shl(to_uint32(b))
}

/// `a >> b`: arithmetic shift, count modulo 32.
pub fn shr(a: f64, b: f64) -> i32 {
    to_int32(a).wrapping_shr(to_uint32(b))
}

/// `a >>> b`: logical shift, count modulo 32; the result is a uint32.
pub fn ushr(a: f64, b: f64) -> u32 {
    to_uint32(a).wrapping_shr(to_uint32(b))
}

/// `a << b` on int32 operands.
#[expect(
    clippy::cast_sign_loss,
    reason = "only the low 5 bits of the count matter"
)]
pub fn shl_i32(a: i32, b: i32) -> i32 {
    a.wrapping_shl(b as u32)
}

/// `a >> b` on int32 operands.
#[expect(
    clippy::cast_sign_loss,
    reason = "only the low 5 bits of the count matter"
)]
pub fn shr_i32(a: i32, b: i32) -> i32 {
    a.wrapping_shr(b as u32)
}

/// `a >>> b` on int32 operands; the result is a uint32.
#[expect(
    clippy::cast_sign_loss,
    reason = "reinterpreting the bits is ToUint32 for an int32"
)]
pub fn ushr_i32(a: i32, b: i32) -> u32 {
    (a as u32).wrapping_shr(b as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_division_and_remainder() {
        assert_eq!(div_trunc_i32(7, -2), -3);
        assert_eq!(div_trunc_i32(i32::MIN, -1), i32::MIN);
        assert_eq!(div_trunc_i32(5, 0), 0);
        assert!(rem_i32(-4, 2).is_sign_negative() && rem_i32(-4, 2) == 0.0);
        assert!(rem_i32(1, 0).is_nan());
        assert_eq!(rem_i32(-7, 3), -1.0);
    }

    #[test]
    fn shifts_mask_the_count() {
        assert_eq!(shl(1.0, 33.0), 2);
        assert_eq!(shl_i32(1, -1), i32::MIN);
        assert_eq!(ushr(-1.0, 0.0), u32::MAX);
        assert_eq!(ushr_i32(-1, 28), 15);
        assert_eq!(shr(-16.0, 2.0), -4);
    }
}
