//! `Math.*` with JavaScript's exact semantics.
//!
//! Everything except the transcendental functions is exact: bit-for-bit what V8 returns,
//! including NaN, ±0 and ±Infinity, which the golden tables check.
//!
//! The transcendental functions (`sin`, `exp`, `log`, `pow`, …) come from `libm`, a port of
//! musl, while V8 uses its own port of fdlibm. ECMAScript only asks for
//! implementation-approximated results, and the two libraries can differ in the last bit for
//! some inputs. Their special cases (NaN, ±0, ±Infinity, domain errors) agree exactly, and
//! [`pow`] applies JavaScript's rules where C's differ. The golden tables record which
//! functions matched V8 exactly on every tested input and bound the rest to 1 ulp; see
//! `tests/golden.rs`. The lowering only moves code using these functions to WASM when the
//! last bit cannot change the program's output, or when the user accepts it.

use crate::{conv, fpu};

/// The transcendental functions whose results match V8's bit for bit: on every input of the
/// golden tables and on the ~2M generated inputs per function behind `tests/golden/wide.txt`
/// (checked against V8 12.4.254.21, Node 22.22). The lowering moves only these, and the exact
/// non-transcendental functions, to WASM without the user opting in.
pub const EXACT_TRANSCENDENTALS: &[&str] = &["tan", "asin", "acos", "atan", "expm1", "log2"];

const TWO_POW_52: f64 = 4_503_599_627_370_496.0;

/// `Math.abs`.
pub fn abs(x: f64) -> f64 {
    x.abs()
}

/// `Math.floor`.
pub fn floor(x: f64) -> f64 {
    fpu::floor(x)
}

/// `Math.ceil`.
pub fn ceil(x: f64) -> f64 {
    fpu::ceil(x)
}

/// `Math.trunc`.
pub fn trunc(x: f64) -> f64 {
    fpu::trunc(x)
}

/// `Math.sqrt` (correctly rounded, so exact).
pub fn sqrt(x: f64) -> f64 {
    fpu::sqrt(x)
}

/// `Math.round`: the nearest integer, ties toward +Infinity. Results in [-0.5, -0] are -0.
///
/// Not `floor(x + 0.5)`, which is wrong for 0.49999999999999994 and near 2^52.
pub fn round(x: f64) -> f64 {
    if !(x.abs() < TWO_POW_52) {
        // NaN, ±Infinity, and values that are already integers.
        return x;
    }
    let floor = fpu::floor(x);
    let rounded = if x - floor >= 0.5 { floor + 1.0 } else { floor };
    if rounded == 0.0 && x.is_sign_negative() {
        -0.0
    } else {
        rounded
    }
}

/// `Math.sign`: NaN, ±0 are returned unchanged.
pub fn sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        x
    }
}

/// `Math.max(a, b)`: NaN if either is NaN, and +0 is larger than -0.
///
/// `Math.max(a, b, c, …)` is `max(max(a, b), c)…`; with no arguments it is
/// `f64::NEG_INFINITY`. See also [`max_of`].
pub fn max(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else if a < b {
        b
    } else if a == b {
        if a.is_sign_positive() { a } else { b }
    } else {
        f64::NAN
    }
}

/// `Math.min(a, b)`: NaN if either is NaN, and -0 is smaller than +0. See also [`min_of`].
pub fn min(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else if a > b {
        b
    } else if a == b {
        if a.is_sign_negative() { a } else { b }
    } else {
        f64::NAN
    }
}

/// `Math.max(...values)`.
pub fn max_of(values: &[f64]) -> f64 {
    values.iter().fold(f64::NEG_INFINITY, |acc, &v| max(acc, v))
}

/// `Math.min(...values)`.
pub fn min_of(values: &[f64]) -> f64 {
    values.iter().fold(f64::INFINITY, |acc, &v| min(acc, v))
}

/// `Math.fround`: round to the nearest `f32`, ties to even.
#[expect(
    clippy::cast_possible_truncation,
    reason = "rounding to f32 is the point"
)]
pub fn fround(x: f64) -> f64 {
    f64::from(x as f32)
}

/// `Math.clz32`: leading zero bits of ToUint32(x).
pub fn clz32(x: f64) -> u32 {
    conv::to_uint32(x).leading_zeros()
}

/// `Math.imul`: the low 32 bits of ToInt32(a) × ToInt32(b), as an int32.
pub fn imul(a: f64, b: f64) -> i32 {
    imul_i32(conv::to_int32(a), conv::to_int32(b))
}

/// `Math.imul` on values already proven to be int32.
pub fn imul_i32(a: i32, b: i32) -> i32 {
    a.wrapping_mul(b)
}

/// `Math.hypot(a, b)`.
pub fn hypot2(a: f64, b: f64) -> f64 {
    hypot(&[a, b])
}

/// `Math.hypot(a, b, c)`.
pub fn hypot3(a: f64, b: f64, c: f64) -> f64 {
    hypot(&[a, b, c])
}

/// `Math.hypot(...values)`, with V8's algorithm so results match bit for bit: ±Infinity wins
/// over NaN, then the values are scaled by the largest magnitude and summed with Kahan
/// compensation. (libm's `hypot` rounds differently.)
pub fn hypot(values: &[f64]) -> f64 {
    let mut max = 0.0_f64;
    let mut any_nan = false;
    for &value in values {
        if value.is_nan() {
            any_nan = true;
        } else if value.abs() > max {
            max = value.abs();
        }
    }
    if max == f64::INFINITY {
        return f64::INFINITY;
    }
    if any_nan {
        return f64::NAN;
    }
    if max == 0.0 {
        return 0.0;
    }
    let mut sum = 0.0;
    let mut compensation = 0.0;
    for &value in values {
        let n = value.abs() / max;
        let summand = n * n - compensation;
        let preliminary = sum + summand;
        compensation = (preliminary - sum) - summand;
        sum = preliminary;
    }
    fpu::sqrt(sum) * max
}

/// `Math.pow(x, y)` and `x ** y`. JavaScript differs from C's `pow` when `y` is NaN (always
/// NaN, even for `x = 1`) and when |x| = 1 and `y` is ±Infinity (NaN, not 1).
pub fn pow(x: f64, y: f64) -> f64 {
    if y.is_nan() || (x.abs() == 1.0 && y.is_infinite()) {
        f64::NAN
    } else {
        libm::pow(x, y)
    }
}

macro_rules! transcendental {
    ($($(#[$doc:meta])* $name:ident($($arg:ident),+);)*) => {$(
        $(#[$doc])*
        ///
        /// Via `libm`; see the module documentation on last-bit differences.
        pub fn $name($($arg: f64),+) -> f64 {
            libm::$name($($arg),+)
        }
    )*};
}

transcendental! {
    /// `Math.sin`.
    sin(x);
    /// `Math.cos`.
    cos(x);
    /// `Math.tan`.
    tan(x);
    /// `Math.asin`.
    asin(x);
    /// `Math.acos`.
    acos(x);
    /// `Math.atan`.
    atan(x);
    /// `Math.atan2`.
    atan2(y, x);
    /// `Math.sinh`.
    sinh(x);
    /// `Math.cosh`.
    cosh(x);
    /// `Math.tanh`.
    tanh(x);
    /// `Math.asinh`.
    asinh(x);
    /// `Math.acosh`.
    acosh(x);
    /// `Math.atanh`.
    atanh(x);
    /// `Math.exp`.
    exp(x);
    /// `Math.expm1`.
    expm1(x);
    /// `Math.log`.
    log(x);
    /// `Math.log1p`.
    log1p(x);
    /// `Math.log2`.
    log2(x);
    /// `Math.log10`.
    log10(x);
    /// `Math.cbrt`.
    cbrt(x);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: f64, b: f64) -> bool {
        a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
    }

    #[test]
    fn round_edges() {
        for (x, expected) in [
            (-0.5, -0.0),
            (0.5, 1.0),
            (-1.5, -1.0),
            (2.5, 3.0),
            (0.499_999_999_999_999_94, 0.0),
            (-0.0, -0.0),
            (-0.2, -0.0),
            (4_503_599_627_370_495.5, 4_503_599_627_370_496.0),
        ] {
            assert!(same(round(x), expected), "round({x}) = {}", round(x));
        }
    }

    #[test]
    fn max_min_zero_and_nan() {
        assert!(same(max(-0.0, 0.0), 0.0) && same(max(0.0, -0.0), 0.0));
        assert!(same(min(-0.0, 0.0), -0.0) && same(min(0.0, -0.0), -0.0));
        assert!(max(f64::NAN, 1.0).is_nan() && min(1.0, f64::NAN).is_nan());
        assert!(same(max_of(&[]), f64::NEG_INFINITY) && same(min_of(&[]), f64::INFINITY));
    }

    #[test]
    fn hypot_infinity_beats_nan() {
        assert!(same(hypot2(f64::NAN, f64::NEG_INFINITY), f64::INFINITY));
        assert!(hypot2(f64::NAN, 1.0).is_nan());
        assert!(same(hypot(&[]), 0.0) && same(hypot2(-0.0, -0.0), 0.0));
    }
}
