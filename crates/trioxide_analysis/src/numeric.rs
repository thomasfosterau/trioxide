//! Interval arithmetic over [`NumberType`], following IEEE-754 double semantics as JavaScript
//! uses them (PLAN §3, numeric range and integer analysis).
//!
//! Every operation is an over-approximation: the result type contains every value the JS
//! operation can produce for operands in the input types. Rounding is monotone, so applying the
//! operation to interval endpoints gives exact bounds; NaN and `-0` are tracked by flags because
//! intervals can't express them.

use trioxide_types::{Interval, NumberType};

const INT32_MIN: f64 = -2_147_483_648.0;
const INT32_MAX: f64 = 2_147_483_647.0;
const UINT32_MAX: f64 = 4_294_967_295.0;

/// The largest valid array length.
pub const MAX_ARRAY_LENGTH: f64 = UINT32_MAX;
/// The largest valid string length (`2^53 - 1`).
pub const MAX_STRING_LENGTH: f64 = 9_007_199_254_740_991.0;

fn int(min: f64, max: f64) -> NumberType {
    NumberType {
        range: Interval::new(min, max),
        int: true,
        non_nan: true,
        non_neg_zero: true,
    }
}

/// Every int32.
pub const fn int32() -> NumberType {
    NumberType {
        range: Interval::new(INT32_MIN, INT32_MAX),
        int: true,
        non_nan: true,
        non_neg_zero: true,
    }
}

/// Every uint32.
pub const fn uint32() -> NumberType {
    NumberType {
        range: Interval::new(0.0, UINT32_MAX),
        int: true,
        non_nan: true,
        non_neg_zero: true,
    }
}

/// An integer in `[min, max]`, never NaN or `-0`.
pub fn int_range(min: f64, max: f64) -> NumberType {
    int(min, max)
}

/// `true` or `false` as a number (`+true`).
pub fn boolean() -> NumberType {
    int(0.0, 1.0)
}

/// Whether `n` may be zero (of either sign).
fn may_be_zero(n: NumberType) -> bool {
    n.range.min <= 0.0 && n.range.max >= 0.0
}

fn may_be_infinite(n: NumberType) -> bool {
    n.range.min == f64::NEG_INFINITY || n.range.max == f64::INFINITY
}

/// The hull of finite candidate values, ignoring NaN candidates (reported separately).
fn hull(values: &[f64]) -> Interval {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for &value in values {
        if !value.is_nan() {
            min = min.min(value);
            max = max.max(value);
        }
    }
    if min > max {
        Interval::FULL
    } else {
        Interval::new(min, max)
    }
}

/// `a + b` on numbers.
pub fn add(a: NumberType, b: NumberType) -> NumberType {
    let min = a.range.min + b.range.min;
    let max = a.range.max + b.range.max;
    // inf + -inf is NaN.
    let inf_cancel = (a.range.max == f64::INFINITY && b.range.min == f64::NEG_INFINITY)
        || (a.range.min == f64::NEG_INFINITY && b.range.max == f64::INFINITY);
    NumberType {
        range: if min.is_nan() || max.is_nan() {
            Interval::FULL
        } else {
            Interval::new(min, max)
        },
        int: a.int && b.int,
        non_nan: a.non_nan && b.non_nan && !inf_cancel,
        // x + y is -0 only when both are -0.
        non_neg_zero: a.non_neg_zero || b.non_neg_zero,
    }
}

/// `-a`.
pub fn neg(a: NumberType) -> NumberType {
    NumberType {
        range: Interval::new(-a.range.max, -a.range.min),
        int: a.int,
        non_nan: a.non_nan,
        // -(+0) is -0.
        non_neg_zero: !may_be_zero(a),
    }
}

/// `a - b`.
pub fn sub(a: NumberType, b: NumberType) -> NumberType {
    let mut result = add(a, neg(b));
    // x - y is -0 only when x is -0 and y is +0.
    result.non_neg_zero = a.non_neg_zero || !may_be_zero(b);
    result
}

/// `a * b`.
pub fn mul(a: NumberType, b: NumberType) -> NumberType {
    let candidates = [
        a.range.min * b.range.min,
        a.range.min * b.range.max,
        a.range.max * b.range.min,
        a.range.max * b.range.max,
    ];
    let zero_times_inf =
        (may_be_zero(a) && may_be_infinite(b)) || (may_be_zero(b) && may_be_infinite(a));
    let mut range = hull(&candidates);
    // A zero operand makes the product zero; intervals through zero already contain it.
    if may_be_zero(a) || may_be_zero(b) {
        range = range.hull(Interval::point(0.0));
    }
    let a_nonneg = a.range.min >= 0.0 && a.non_neg_zero;
    let b_nonneg = b.range.min >= 0.0 && b.non_neg_zero;
    // Strictly negative: a `+0` operand would make a negative product's zero `-0`.
    let a_neg = a.range.max < 0.0;
    let b_neg = b.range.max < 0.0;
    NumberType {
        range,
        int: a.int && b.int,
        non_nan: a.non_nan && b.non_nan && !zero_times_inf,
        // The sign of a zero product is the XOR of the signs; underflow keeps the sign too.
        non_neg_zero: (a_nonneg && b_nonneg) || (a_neg && b_neg),
    }
}

/// `a / b`.
pub fn div(a: NumberType, b: NumberType) -> NumberType {
    let a_nonneg = a.range.min >= 0.0 && a.non_neg_zero;
    let b_nonneg = b.range.min >= 0.0 && b.non_neg_zero;
    let non_nan = a.non_nan
        && b.non_nan
        && !(may_be_zero(a) && may_be_zero(b))
        && !(may_be_infinite(a) && may_be_infinite(b));
    // With the divisor's sign fixed, division is monotone in each operand.
    let range = if b.range.min > 0.0 || b.range.max < 0.0 {
        hull(&[
            a.range.min / b.range.min,
            a.range.min / b.range.max,
            a.range.max / b.range.min,
            a.range.max / b.range.max,
        ])
    } else if a_nonneg && b_nonneg {
        Interval::new(0.0, f64::INFINITY)
    } else {
        Interval::FULL
    };
    NumberType {
        range,
        int: false,
        non_nan,
        // A zero quotient takes the XOR of the signs.
        non_neg_zero: (a_nonneg && b_nonneg) || (a.range.max < 0.0 && b.range.max < 0.0),
    }
}

/// `a % b` (sign follows the dividend).
pub fn rem(a: NumberType, b: NumberType) -> NumberType {
    let bound = b.range.min.abs().max(b.range.max.abs());
    let non_nan = a.non_nan && b.non_nan && !may_be_zero(b) && !may_be_infinite(a);
    let mut min = if a.range.min >= 0.0 { 0.0 } else { -bound };
    let mut max = if a.range.max <= 0.0 { 0.0 } else { bound };
    // |a % b| <= |a| too.
    min = min.max(a.range.min.min(0.0));
    max = max.min(a.range.max.max(0.0));
    let int = a.int && b.int;
    if int && bound.is_finite() && bound > 0.0 {
        // |a % b| <= |b| - 1 for integers.
        min = min.max(-(bound - 1.0));
        max = max.min(bound - 1.0);
    }
    NumberType {
        range: Interval::new(min, max),
        int,
        non_nan,
        non_neg_zero: a.range.min >= 0.0 && a.non_neg_zero,
    }
}

/// `a ** b`, which is only bounded in easy cases.
pub fn pow(a: NumberType, b: NumberType) -> NumberType {
    let base_nonneg = a.range.min >= 0.0 && a.non_neg_zero;
    NumberType {
        range: if base_nonneg {
            Interval::new(0.0, f64::INFINITY)
        } else {
            Interval::FULL
        },
        int: a.int && b.int && b.range.min >= 0.0,
        // NaN ** 0 is 1 but 1 ** NaN, (-2) ** 0.5 and 1 ** Infinity are NaN.
        non_nan: false,
        non_neg_zero: base_nonneg,
    }
}

/// `ToInt32(a)`.
pub fn to_int32(a: NumberType) -> NumberType {
    if a.int && a.non_nan && a.range.min >= INT32_MIN && a.range.max <= INT32_MAX {
        int(a.range.min, a.range.max)
    } else {
        int32()
    }
}

/// `a | b`.
pub fn bit_or(a: NumberType, b: NumberType) -> NumberType {
    let (a, b) = (to_int32(a), to_int32(b));
    if b.as_constant() == Some(0.0) {
        return a;
    }
    if a.as_constant() == Some(0.0) {
        return b;
    }
    if a.range.min >= 0.0 && b.range.min >= 0.0 {
        // Both non-negative: the result has no bit above the larger operand's top bit.
        let top = next_pow2_minus1(a.range.max.max(b.range.max));
        return int(a.range.min.max(b.range.min), top);
    }
    int32()
}

/// `a & b`.
pub fn bit_and(a: NumberType, b: NumberType) -> NumberType {
    let (a, b) = (to_int32(a), to_int32(b));
    match (a.range.min >= 0.0, b.range.min >= 0.0) {
        (true, true) => int(0.0, a.range.max.min(b.range.max)),
        (true, false) => int(0.0, a.range.max),
        (false, true) => int(0.0, b.range.max),
        (false, false) => int32(),
    }
}

/// `a ^ b`.
pub fn bit_xor(a: NumberType, b: NumberType) -> NumberType {
    let (a, b) = (to_int32(a), to_int32(b));
    if a.range.min >= 0.0 && b.range.min >= 0.0 {
        return int(0.0, next_pow2_minus1(a.range.max.max(b.range.max)));
    }
    int32()
}

/// `~a`.
pub fn bit_not(a: NumberType) -> NumberType {
    let a = to_int32(a);
    int(-a.range.max - 1.0, -a.range.min - 1.0)
}

/// `a << b`.
pub fn shl(a: NumberType, b: NumberType) -> NumberType {
    let (a, shift) = (to_int32(a), shift_count(b));
    if let Some(shift) = shift
        && a.range.min >= 0.0
    {
        let max = a.range.max * f64::from(1u32 << shift);
        if max <= INT32_MAX {
            return int(a.range.min * f64::from(1u32 << shift), max);
        }
    }
    int32()
}

/// `a >> b`.
pub fn shr(a: NumberType, b: NumberType) -> NumberType {
    let a = to_int32(a);
    match shift_count(b) {
        Some(shift) => {
            let factor = f64::from(1u32 << shift);
            int(
                (a.range.min / factor).floor(),
                (a.range.max / factor).floor(),
            )
        }
        #[expect(
            clippy::manual_clamp,
            reason = "`min`/`max` ignore a NaN bound where `clamp` would propagate it"
        )]
        None => int(a.range.min.min(-1.0).max(INT32_MIN), a.range.max.max(0.0)),
    }
}

/// `a >>> b`.
pub fn ushr(a: NumberType, b: NumberType) -> NumberType {
    let shift = shift_count(b);
    if a.int && a.non_nan && a.range.min >= 0.0 && a.range.max <= UINT32_MAX {
        return match shift {
            Some(shift) => {
                let factor = f64::from(1u32 << shift);
                int(
                    (a.range.min / factor).floor(),
                    (a.range.max / factor).floor(),
                )
            }
            None => int(0.0, a.range.max),
        };
    }
    match shift {
        Some(shift) if shift > 0 => int(0.0, (UINT32_MAX / f64::from(1u32 << shift)).floor()),
        _ => uint32(),
    }
}

/// The shift count `b & 31`, if it is a known constant.
fn shift_count(b: NumberType) -> Option<u32> {
    let value = b.as_constant()?;
    if !value.is_finite() || value.fract() != 0.0 {
        return None;
    }
    // `rem_euclid` is exact on doubles, so this is `ToUint32(value) & 31`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "an integer in [0, 32)"
    )]
    let count = value.rem_euclid(32.0) as u32;
    Some(count)
}

/// The smallest `2^k - 1 >= value`, for non-negative int32 values.
fn next_pow2_minus1(value: f64) -> f64 {
    let mut top = 1.0;
    while top - 1.0 < value {
        top *= 2.0;
    }
    top - 1.0
}

/// `Math.floor`.
pub fn floor(a: NumberType) -> NumberType {
    NumberType {
        range: Interval::new(a.range.min.floor(), a.range.max.floor()),
        int: true,
        non_nan: a.non_nan,
        non_neg_zero: a.non_neg_zero,
    }
}

/// `Math.ceil`: `ceil(-0.5)` is `-0`.
pub fn ceil(a: NumberType) -> NumberType {
    NumberType {
        range: Interval::new(a.range.min.ceil(), a.range.max.ceil()),
        int: true,
        non_nan: a.non_nan,
        non_neg_zero: (a.range.min >= 0.0 && a.non_neg_zero) || a.range.max <= -1.0,
    }
}

/// `Math.trunc`.
pub fn trunc(a: NumberType) -> NumberType {
    NumberType {
        range: Interval::new(a.range.min.trunc(), a.range.max.trunc()),
        int: true,
        non_nan: a.non_nan,
        non_neg_zero: (a.range.min >= 0.0 && a.non_neg_zero) || a.range.max <= -1.0,
    }
}

/// `Math.round`: rounds half up, and `round(-0.4)` is `-0`.
pub fn round(a: NumberType) -> NumberType {
    NumberType {
        // floor(x) <= round(x) <= ceil(x); `x + 0.5` itself rounds for large x.
        range: Interval::new(a.range.min.floor(), a.range.max.ceil()),
        int: true,
        non_nan: a.non_nan,
        non_neg_zero: (a.range.min >= 0.0 && a.non_neg_zero) || a.range.max < -0.5,
    }
}

/// `Math.abs`.
pub fn abs(a: NumberType) -> NumberType {
    let (min, max) = if a.range.min >= 0.0 {
        (a.range.min, a.range.max)
    } else if a.range.max <= 0.0 {
        (-a.range.max, -a.range.min)
    } else {
        (0.0, a.range.max.max(-a.range.min))
    };
    NumberType {
        range: Interval::new(min, max),
        int: a.int,
        non_nan: a.non_nan,
        non_neg_zero: true,
    }
}

/// `Math.sqrt`.
pub fn sqrt(a: NumberType) -> NumberType {
    NumberType {
        range: Interval::new(a.range.min.max(0.0).sqrt(), a.range.max.max(0.0).sqrt()),
        int: false,
        non_nan: a.non_nan && a.range.min >= 0.0,
        non_neg_zero: a.non_neg_zero,
    }
}

/// `Math.sign`.
pub fn sign(a: NumberType) -> NumberType {
    let min = if a.range.min < 0.0 {
        -1.0
    } else if a.range.min > 0.0 {
        1.0
    } else {
        0.0
    };
    let max = if a.range.max > 0.0 {
        1.0
    } else if a.range.max < 0.0 {
        -1.0
    } else {
        0.0
    };
    NumberType {
        range: Interval::new(min, max),
        int: true,
        non_nan: a.non_nan,
        non_neg_zero: a.non_neg_zero,
    }
}

/// `Math.min(a, b)` (`Math.max` with `max = true`): NaN if either is, and `-0 < +0`.
pub fn min_max(a: NumberType, b: NumberType, max: bool) -> NumberType {
    let range = if max {
        Interval::new(a.range.min.max(b.range.min), a.range.max.max(b.range.max))
    } else {
        Interval::new(a.range.min.min(b.range.min), a.range.max.min(b.range.max))
    };
    NumberType {
        range,
        int: a.int && b.int,
        non_nan: a.non_nan && b.non_nan,
        // The result is one of the operands.
        non_neg_zero: a.non_neg_zero && b.non_neg_zero,
    }
}

/// A value in `[-1, 1]` that is NaN for infinite or NaN inputs (`Math.sin`, `Math.cos`).
pub fn unit_wave(a: NumberType) -> NumberType {
    NumberType {
        range: Interval::new(-1.0, 1.0),
        int: false,
        non_nan: a.non_nan && !may_be_infinite(a),
        non_neg_zero: false,
    }
}

/// Narrows `a` given that `a < b` (`strict`) or `a <= b` held.
pub fn assume_less(a: NumberType, b: NumberType, strict: bool) -> NumberType {
    let mut max = b.range.max;
    if strict && a.int && max.is_finite() {
        // An integer below `max` is at most `ceil(max) - 1`.
        max = max.ceil() - 1.0;
    }
    NumberType {
        range: Interval::new(a.range.min, a.range.max.min(max)),
        // A comparison that held had no NaN operand.
        non_nan: true,
        ..a
    }
}

/// Narrows `a` given that `a > b` (`strict`) or `a >= b` held.
pub fn assume_greater(a: NumberType, b: NumberType, strict: bool) -> NumberType {
    let mut min = b.range.min;
    if strict && a.int && min.is_finite() {
        min = min.floor() + 1.0;
    }
    NumberType {
        range: Interval::new(a.range.min.max(min), a.range.max),
        non_nan: true,
        ..a
    }
}

/// Widens `next` against `previous`: any bound that moved goes to infinity.
pub fn widen(previous: NumberType, next: NumberType) -> NumberType {
    let joined = previous.join(next);
    NumberType {
        range: Interval::new(
            if joined.range.min < previous.range.min {
                f64::NEG_INFINITY
            } else {
                joined.range.min
            },
            if joined.range.max > previous.range.max {
                f64::INFINITY
            } else {
                joined.range.max
            },
        ),
        ..joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(value: f64) -> NumberType {
        NumberType::constant(value)
    }

    fn contains(n: NumberType, value: f64) -> bool {
        if value.is_nan() {
            return !n.non_nan;
        }
        if value == 0.0 && value.is_sign_negative() && n.non_neg_zero {
            return false;
        }
        if n.int && value.is_finite() && value.fract() != 0.0 {
            return false;
        }
        n.range.min <= value && value <= n.range.max
    }

    const SAMPLES: [f64; 17] = [
        f64::NEG_INFINITY,
        -1e300,
        -65536.5,
        -3.0,
        -1.0,
        -0.5,
        -1e-300,
        -0.0,
        0.0,
        1e-300,
        0.5,
        1.0,
        2.0,
        7.0,
        4_294_967_296.0,
        1e300,
        f64::INFINITY,
    ];

    /// Every operation contains the exact result for constant operands.
    #[test]
    fn binary_operations_contain_their_results() {
        type Op = (
            fn(NumberType, NumberType) -> NumberType,
            fn(f64, f64) -> f64,
        );
        #[expect(clippy::cast_sign_loss, reason = "JS semantics")]
        let ops: [(&str, Op); 13] = [
            ("+", (add, |a, b| a + b)),
            ("-", (sub, |a, b| a - b)),
            ("*", (mul, |a, b| a * b)),
            ("/", (div, |a, b| a / b)),
            ("%", (rem, |a, b| a % b)),
            ("**", (pow, f64::powf)),
            ("|", (bit_or, |a, b| f64::from(js_int32(a) | js_int32(b)))),
            ("&", (bit_and, |a, b| f64::from(js_int32(a) & js_int32(b)))),
            ("^", (bit_xor, |a, b| f64::from(js_int32(a) ^ js_int32(b)))),
            (
                "<<",
                (shl, |a, b| {
                    f64::from(js_int32(a).wrapping_shl(js_int32(b) as u32 & 31))
                }),
            ),
            (
                ">>",
                (shr, |a, b| {
                    f64::from(js_int32(a) >> (js_int32(b) as u32 & 31))
                }),
            ),
            (
                ">>>",
                (ushr, |a, b| {
                    f64::from((js_int32(a) as u32) >> (js_int32(b) as u32 & 31))
                }),
            ),
            ("max", (|a, b| min_max(a, b, true), js_max)),
        ];
        for (name, (op, exact)) in ops {
            for &a in &SAMPLES {
                for &b in &SAMPLES {
                    let result = op(c(a), c(b));
                    let expected = if name == "**" {
                        js_pow(a, b)
                    } else {
                        exact(a, b)
                    };
                    assert!(
                        contains(result, expected),
                        "{a} {name} {b} = {expected}, not in {result:?}"
                    );
                    // And for the hull of two operands.
                    let wide = op(c(a).join(c(-1.0)), c(b).join(c(3.0)));
                    assert!(contains(wide, expected), "{a} {name} {b} (hull)");
                }
            }
        }
    }

    #[test]
    fn unary_operations_contain_their_results() {
        #[expect(clippy::type_complexity, reason = "a table of test cases")]
        let ops: [(&str, fn(NumberType) -> NumberType, fn(f64) -> f64); 8] = [
            ("neg", neg, |a| -a),
            ("floor", floor, f64::floor),
            ("ceil", ceil, f64::ceil),
            ("trunc", trunc, f64::trunc),
            ("round", round, js_round),
            ("abs", abs, f64::abs),
            ("sqrt", sqrt, f64::sqrt),
            ("sign", sign, js_sign),
        ];
        for (name, op, exact) in ops {
            for &a in &SAMPLES {
                let expected = exact(a);
                assert!(contains(op(c(a)), expected), "{name}({a}) = {expected}");
                assert!(
                    contains(op(c(a).join(c(0.25))), expected),
                    "{name}({a}) hull"
                );
            }
        }
    }

    #[test]
    fn loop_counters() {
        // i in [0, len) with len <= 2^32 - 1.
        let length = int_range(0.0, MAX_ARRAY_LENGTH);
        let i = assume_less(int_range(0.0, f64::INFINITY), length, true);
        assert_eq!(i.range, Interval::new(0.0, MAX_ARRAY_LENGTH - 1.0));
        assert!(i.int && i.non_nan);
        assert_eq!(add(i, c(1.0)).range.max, MAX_ARRAY_LENGTH);
        assert_eq!(bit_or(c(-5.5), c(0.0)), int32());
        assert_eq!(bit_or(int_range(0.0, 10.0), c(0.0)), int_range(0.0, 10.0));
        assert_eq!(bit_and(NumberType::ANY, c(255.0)), int_range(0.0, 255.0));
        assert_eq!(ushr(NumberType::ANY, c(0.0)), uint32());
    }

    fn js_int32(value: f64) -> i32 {
        if !value.is_finite() {
            return 0;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "modular conversion of a non-negative value"
        )]
        let wrapped = (value.trunc().rem_euclid(4_294_967_296.0)) as u64 as u32;
        #[expect(clippy::cast_possible_wrap, reason = "two's complement")]
        let signed = wrapped as i32;
        signed
    }

    fn js_max(a: f64, b: f64) -> f64 {
        if a.is_nan() || b.is_nan() {
            return f64::NAN;
        }
        if a == 0.0 && b == 0.0 {
            return if a.is_sign_positive() || b.is_sign_positive() {
                0.0
            } else {
                -0.0
            };
        }
        a.max(b)
    }

    #[expect(clippy::float_cmp, reason = "exact JS semantics")]
    fn js_pow(a: f64, b: f64) -> f64 {
        if b.is_nan() || (a.abs() == 1.0 && b.is_infinite()) {
            return f64::NAN;
        }
        a.powf(b)
    }

    fn js_round(a: f64) -> f64 {
        if a.is_nan() || a.is_infinite() {
            return a;
        }
        let rounded = (a + 0.5).floor();
        if rounded == 0.0 && a.is_sign_negative() {
            -0.0
        } else if (a + 0.5).floor() - a > 0.5 {
            rounded - 1.0
        } else {
            rounded
        }
    }

    fn js_sign(a: f64) -> f64 {
        if a.is_nan() || a == 0.0 {
            a
        } else {
            a.signum()
        }
    }
}

#[cfg(test)]
#[allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
mod review_props {
    use super::*;

    const POOL: [f64; 34] = [
        f64::NEG_INFINITY,
        -1e300,
        -9007199254740994.0,
        -4294967296.0,
        -2147483649.0,
        -2147483648.0,
        -65536.5,
        -3.0,
        -2.5,
        -1.0,
        -0.75,
        -0.5,
        -0.49999999999999994,
        -0.4,
        -1e-300,
        -5e-324,
        -0.0,
        0.0,
        5e-324,
        1e-300,
        0.4,
        0.49999999999999994,
        0.5,
        1.0,
        2.5,
        3.0,
        31.0,
        32.0,
        2147483647.0,
        2147483648.0,
        4294967295.0,
        9007199254740993.0,
        1e300,
        f64::INFINITY,
    ];

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn pick(&mut self) -> f64 {
            POOL[(self.next() % POOL.len() as u64) as usize]
        }
    }

    /// A random set of concrete values, and the tightest abstraction of it.
    fn sample(rng: &mut Rng) -> (Vec<f64>, NumberType) {
        let n = 1 + rng.next() % 4;
        let mut vals: Vec<f64> = (0..n).map(|_| rng.pick()).collect();
        if rng.next() % 6 == 0 {
            vals.push(f64::NAN);
        }
        // Also interior points of the hull.
        let finite: Vec<f64> = vals.iter().copied().filter(|v| v.is_finite()).collect();
        if finite.len() >= 2 {
            let (lo, hi) = (
                finite.iter().cloned().fold(f64::INFINITY, f64::min),
                finite.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            );
            let mid = lo / 2.0 + hi / 2.0;
            vals.push(mid);
            if rng.next() % 2 == 0 && (hi - lo) > 2.0 {
                vals.push(mid.round());
            }
        }
        let mut t = NumberType::constant(vals[0]);
        for &v in &vals[1..] {
            t = t.join(NumberType::constant(v));
        }
        // interior values must respect int-ness of the abstraction
        if t.int {
            vals.retain(|v| !v.is_finite() || v.fract() == 0.0);
        }
        (vals, t)
    }

    fn contains(n: NumberType, value: f64) -> bool {
        if value.is_nan() {
            return !n.non_nan;
        }
        if value == 0.0 && value.is_sign_negative() && n.non_neg_zero {
            return false;
        }
        if n.int && value.is_finite() && value.fract() != 0.0 {
            return false;
        }
        n.range.min <= value && value <= n.range.max
    }

    fn i32_(v: f64) -> i32 {
        if !v.is_finite() {
            return 0;
        }
        (v.trunc().rem_euclid(4294967296.0)) as u64 as u32 as i32
    }
    fn js_pow(a: f64, b: f64) -> f64 {
        if b.is_nan() || (a.abs() == 1.0 && b.is_infinite()) {
            return f64::NAN;
        }
        a.powf(b)
    }
    fn js_round(x: f64) -> f64 {
        if !x.is_finite() || x == 0.0 {
            return x;
        }
        if x < 0.0 && x >= -0.5 {
            return -0.0;
        }
        let f = x.floor();
        let r = if x - f >= 0.5 { f + 1.0 } else { f };
        if r == 0.0 && x < 0.0 { -0.0 } else { r }
    }
    fn js_minmax(a: f64, b: f64, max: bool) -> f64 {
        if a.is_nan() || b.is_nan() {
            return f64::NAN;
        }
        if a == 0.0 && b == 0.0 {
            let neg = if max {
                a.is_sign_negative() && b.is_sign_negative()
            } else {
                a.is_sign_negative() || b.is_sign_negative()
            };
            return if neg { -0.0 } else { 0.0 };
        }
        if max { a.max(b) } else { a.min(b) }
    }
    fn js_sign(a: f64) -> f64 {
        if a.is_nan() || a == 0.0 {
            a
        } else {
            a.signum()
        }
    }

    #[test]
    fn random_intervals_binary() {
        type B = (
            &'static str,
            fn(NumberType, NumberType) -> NumberType,
            fn(f64, f64) -> f64,
        );
        let ops: Vec<B> = vec![
            ("+", add, |a, b| a + b),
            ("-", sub, |a, b| a - b),
            ("*", mul, |a, b| a * b),
            ("/", div, |a, b| a / b),
            ("%", rem, |a, b| a % b),
            ("**", pow, js_pow),
            ("|", bit_or, |a, b| f64::from(i32_(a) | i32_(b))),
            ("&", bit_and, |a, b| f64::from(i32_(a) & i32_(b))),
            ("^", bit_xor, |a, b| f64::from(i32_(a) ^ i32_(b))),
            ("<<", shl, |a, b| {
                f64::from(i32_(a).wrapping_shl(i32_(b) as u32 & 31))
            }),
            (">>", shr, |a, b| {
                f64::from(i32_(a) >> (i32_(b) as u32 & 31))
            }),
            (">>>", ushr, |a, b| {
                f64::from((i32_(a) as u32) >> (i32_(b) as u32 & 31))
            }),
            (
                "max",
                |a, b| min_max(a, b, true),
                |a, b| js_minmax(a, b, true),
            ),
            (
                "min",
                |a, b| min_max(a, b, false),
                |a, b| js_minmax(a, b, false),
            ),
        ];
        let mut rng = Rng(0x9E3779B97F4A7C15);
        let mut fails = std::collections::BTreeMap::new();
        for _ in 0..200_000 {
            let (xa, ta) = sample(&mut rng);
            let (xb, tb) = sample(&mut rng);
            for (name, op, exact) in &ops {
                let r = op(ta, tb);
                for &a in &xa {
                    for &b in &xb {
                        let e = exact(a, b);
                        if !contains(r, e) {
                            fails.entry(*name).or_insert_with(|| format!("{a:e} {name} {b:e} = {e:e} not in {r:?}; a in {ta:?}, b in {tb:?}"));
                        }
                    }
                }
            }
        }
        for (_, f) in &fails {
            eprintln!("FAIL {f}");
        }
        assert!(fails.is_empty());
    }

    #[test]
    fn random_intervals_unary() {
        type U = (&'static str, fn(NumberType) -> NumberType, fn(f64) -> f64);
        let ops: Vec<U> = vec![
            ("neg", neg, |a| -a),
            ("floor", floor, f64::floor),
            ("ceil", ceil, f64::ceil),
            ("trunc", trunc, f64::trunc),
            ("round", round, js_round),
            ("abs", abs, f64::abs),
            ("sqrt", sqrt, f64::sqrt),
            ("sign", sign, js_sign),
            ("~", bit_not, |a| f64::from(!i32_(a))),
            ("toint32", to_int32, |a| f64::from(i32_(a))),
            ("sin", unit_wave, f64::sin),
            ("cos", unit_wave, f64::cos),
        ];
        let mut rng = Rng(12345);
        let mut fails = std::collections::BTreeMap::new();
        for _ in 0..200_000 {
            let (xa, ta) = sample(&mut rng);
            for (name, op, exact) in &ops {
                let r = op(ta);
                for &a in &xa {
                    let e = exact(a);
                    if !contains(r, e) {
                        fails.entry(*name).or_insert_with(|| {
                            format!("{name}({a:e}) = {e:e} not in {r:?}; a in {ta:?}")
                        });
                    }
                }
            }
        }
        for (_, f) in &fails {
            eprintln!("FAIL {f}");
        }
        assert!(fails.is_empty());
    }

    #[test]
    fn random_assume() {
        let mut rng = Rng(777);
        let mut fails = Vec::new();
        for _ in 0..100_000 {
            let (xa, ta) = sample(&mut rng);
            let (xb, tb) = sample(&mut rng);
            for strict in [true, false] {
                let lt = assume_less(ta, tb, strict);
                let gt = assume_greater(ta, tb, strict);
                for &a in &xa {
                    for &b in &xb {
                        let holds_lt = if strict { a < b } else { a <= b };
                        let holds_gt = if strict { a > b } else { a >= b };
                        if holds_lt && !contains(lt, a) {
                            fails.push(format!(
                                "assume {a:e} <{} {b:e} but {lt:?}",
                                if strict { "" } else { "=" }
                            ));
                        }
                        if holds_gt && !contains(gt, a) {
                            fails.push(format!(
                                "assume {a:e} >{} {b:e} but {gt:?}",
                                if strict { "" } else { "=" }
                            ));
                        }
                    }
                }
            }
        }
        for f in fails.iter().take(5) {
            eprintln!("FAIL {f}");
        }
        assert!(fails.is_empty());
    }
}
