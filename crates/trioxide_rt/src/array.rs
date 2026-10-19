//! Typed arrays and `number[]` inside an island.
//!
//! Arrays reach Rust as slices into linear memory (see [`abi`](crate::abi)). A typed array of
//! kind `K` is a slice of `K`'s [`Element`] type; a `number[]` is a `[f64]`.
//!
//! - **Reads**: [`get`] / [`get_i32`] / [`get_usize`] return `NaN` for any index that is not an in-bounds
//!   integer, which is what `undefined` becomes in arithmetic. Reads the analysis proves in
//!   bounds use [`get_unchecked`] or plain `xs[i]`. A read whose `undefined` result could be
//!   observed other than through arithmetic (`=== undefined`, `??`) keeps the function in JS.
//! - **`ToInt32` of a read** ([`get_int32`] …) skips the `f64`: out of bounds it is 0.
//! - **In-place `+=` of an int32** ([`add`] …) stays in the element type's arithmetic.
//! - **Typed array writes**: [`set`] / [`set_i32`] / [`set_usize`] convert with the element type's rules and
//!   silently ignore invalid or out-of-bounds indices, as typed arrays do.
//! - **`number[]` writes** must be proven in bounds (an out-of-bounds write would grow the
//!   array, which v1 islands cannot express): use `xs[i] = v` or [`set_unchecked`].
//!   `number[]` never changes length inside an island. Values go through [`store_number`].
//! - **Holes.** A `number[]` element that is `undefined` (a hole) arrives as the signalling
//!   NaN [`HOLE`], which arithmetic turns into an ordinary NaN, as `undefined` does. The
//!   lowering never stores a value that may be `undefined` in a `number[]`, so one with these
//!   bits came through a conversion to number: [`store_number`] makes it NaN, and the glue
//!   leaves elements that still hold [`HOLE`] alone.

use crate::{conv, fpu};

/// An element type of a typed array, with JavaScript's conversion to and from numbers.
pub trait Element: Copy + Default + 'static {
    /// The element as a number.
    fn to_f64(self) -> f64;
    /// The element a typed array stores for `value` (ECMA-262 `NumericToRawBytes`).
    fn from_f64(value: f64) -> Self;

    /// `ToInt32` of the element as a number.
    fn to_int32(self) -> i32 {
        conv::to_int32(self.to_f64())
    }

    /// The element `xs[i] += delta` stores in place of this one: `from_f64(to_f64() + delta)`,
    /// JavaScript's arithmetic, which integer kinds override with their own wrapping
    /// arithmetic (the `f64` sum of an integer element and an int32 is exact).
    #[must_use]
    fn add_int32(self, delta: i32) -> Self {
        Self::from_f64(self.to_f64() + f64::from(delta))
    }
}

/// The element type of a `Uint8ClampedArray`: a byte stored with `ToUint8Clamp` (clamp to
/// 0..=255, round half to even, NaN → 0) instead of wrapping.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct U8Clamped(pub u8);

macro_rules! wrapping_element {
    ($($ty:ty),*) => {$(
        impl Element for $ty {
            fn to_f64(self) -> f64 {
                f64::from(self)
            }

            fn from_f64(value: f64) -> Self {
                // ToInt8, ToUint8, ToInt16, ToUint16: the low bytes of ToInt32.
                Self::low_bytes(conv::to_int32(value))
            }

            fn to_int32(self) -> i32 {
                i32::from(self)
            }

            // The sum modulo 2^32, then its low bytes: what ToInt8 etc. keep of the exact sum.
            fn add_int32(self, delta: i32) -> Self {
                Self::low_bytes(i32::from(self).wrapping_add(delta))
            }
        }

        impl LowBytes for $ty {
            fn low_bytes(value: i32) -> Self {
                let bytes = value.to_le_bytes();
                let mut low = [0; size_of::<$ty>()];
                low.copy_from_slice(&bytes[..size_of::<$ty>()]);
                <$ty>::from_le_bytes(low)
            }
        }
    )*};
}

/// The element type whose bits are the low bytes of an int32.
trait LowBytes {
    fn low_bytes(value: i32) -> Self;
}

wrapping_element!(i8, u8, i16, u16);

impl Element for i32 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }

    fn from_f64(value: f64) -> Self {
        conv::to_int32(value)
    }

    fn to_int32(self) -> i32 {
        self
    }

    fn add_int32(self, delta: i32) -> Self {
        self.wrapping_add(delta)
    }
}

impl Element for u32 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }

    fn from_f64(value: f64) -> Self {
        conv::to_uint32(value)
    }

    #[expect(clippy::cast_possible_wrap, reason = "ToInt32 wraps modulo 2^32")]
    fn to_int32(self) -> i32 {
        self as i32
    }

    fn add_int32(self, delta: i32) -> Self {
        self.wrapping_add_signed(delta)
    }
}

impl Element for f32 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "rounding to f32 is the point"
    )]
    fn from_f64(value: f64) -> Self {
        value as f32
    }
}

impl Element for f64 {
    fn to_f64(self) -> f64 {
        self
    }

    fn from_f64(value: f64) -> Self {
        value
    }
}

impl Element for U8Clamped {
    fn to_f64(self) -> f64 {
        f64::from(self.0)
    }

    fn from_f64(value: f64) -> Self {
        Self(to_uint8_clamp(value))
    }
}

/// `ToUint8Clamp` (ECMA-262 §7.1.12).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is an integer in 0..=255 by then"
)]
pub fn to_uint8_clamp(value: f64) -> u8 {
    if !(value > 0.0) {
        return 0; // also NaN
    }
    if value >= 255.0 {
        return 255;
    }
    let floor = fpu::floor(value);
    let half = floor + 0.5;
    let rounded = if half < value || (half == value && floor % 2.0 != 0.0) {
        floor + 1.0
    } else {
        floor
    };
    rounded as u8
}

/// The index a number denotes, if it is a non-negative integer (-0 counts as 0).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "the round trip through usize is the integer check"
)]
fn index_of(index: f64) -> Option<usize> {
    let i = index as usize;
    (i as f64 == index).then_some(i)
}

/// `xs[index]` in arithmetic: the element, or `NaN` when `index` is not an in-bounds integer.
pub fn get<T: Element>(xs: &[T], index: f64) -> f64 {
    index_of(index)
        .and_then(|i| xs.get(i))
        .map_or(f64::NAN, |x| x.to_f64())
}

/// [`get`] with an int32 index.
pub fn get_i32<T: Element>(xs: &[T], index: i32) -> f64 {
    usize::try_from(index)
        .ok()
        .and_then(|i| xs.get(i))
        .map_or(f64::NAN, |x| x.to_f64())
}

/// [`get`] with a non-negative integer index, e.g. a loop counter bounded by another
/// array's length.
pub fn get_usize<T: Element>(xs: &[T], index: usize) -> f64 {
    xs.get(index).map_or(f64::NAN, |x| x.to_f64())
}

/// `xs[index]` where the analysis proved `index` in bounds.
///
/// # Safety
///
/// `index < xs.len()`.
pub unsafe fn get_unchecked<T: Element>(xs: &[T], index: usize) -> f64 {
    // SAFETY: the caller guarantees `index` is in bounds.
    unsafe { xs.get_unchecked(index) }.to_f64()
}

/// `xs[index] = value` on a typed array: converts `value` to the element type and does
/// nothing if `index` is not an in-bounds integer.
pub fn set<T: Element>(xs: &mut [T], index: f64, value: f64) {
    if let Some(slot) = index_of(index).and_then(|i| xs.get_mut(i)) {
        *slot = T::from_f64(value);
    }
}

/// [`set`] with an int32 index.
pub fn set_i32<T: Element>(xs: &mut [T], index: i32, value: f64) {
    if let Some(slot) = usize::try_from(index).ok().and_then(|i| xs.get_mut(i)) {
        *slot = T::from_f64(value);
    }
}

/// [`set`] with a non-negative integer index.
pub fn set_usize<T: Element>(xs: &mut [T], index: usize, value: f64) {
    if let Some(slot) = xs.get_mut(index) {
        *slot = T::from_f64(value);
    }
}

/// `xs[index] = value` where the analysis proved `index` in bounds.
///
/// # Safety
///
/// `index < xs.len()`.
pub unsafe fn set_unchecked<T: Element>(xs: &mut [T], index: usize, value: f64) {
    // SAFETY: the caller guarantees `index` is in bounds.
    *unsafe { xs.get_unchecked_mut(index) } = T::from_f64(value);
}

/// `ToInt32(xs[index])` (as in `xs[index] | 0`): the element's int32, or 0 when `index` is
/// not an in-bounds integer (`undefined` converts to 0) or the element is a hole.
pub fn get_int32<T: Element>(xs: &[T], index: f64) -> i32 {
    index_of(index)
        .and_then(|i| xs.get(i))
        .map_or(0, |x| x.to_int32())
}

/// [`get_int32`] with an int32 index.
pub fn get_int32_i32<T: Element>(xs: &[T], index: i32) -> i32 {
    usize::try_from(index)
        .ok()
        .and_then(|i| xs.get(i))
        .map_or(0, |x| x.to_int32())
}

/// [`get_int32`] with a non-negative integer index.
pub fn get_int32_usize<T: Element>(xs: &[T], index: usize) -> i32 {
    xs.get(index).map_or(0, |x| x.to_int32())
}

/// `xs[index] += delta` on a typed array, for an int32 `delta`: in the element type's own
/// arithmetic, and nothing when `index` is not an in-bounds integer (JavaScript reads
/// `undefined`, adds to `NaN` and ignores the store).
pub fn add<T: Element>(xs: &mut [T], index: f64, delta: i32) {
    if let Some(slot) = index_of(index).and_then(|i| xs.get_mut(i)) {
        *slot = slot.add_int32(delta);
    }
}

/// [`add`] with an int32 index.
pub fn add_i32<T: Element>(xs: &mut [T], index: i32, delta: i32) {
    if let Some(slot) = usize::try_from(index).ok().and_then(|i| xs.get_mut(i)) {
        *slot = slot.add_int32(delta);
    }
}

/// [`add`] with a non-negative integer index.
pub fn add_usize<T: Element>(xs: &mut [T], index: usize, delta: i32) {
    if let Some(slot) = xs.get_mut(index) {
        *slot = slot.add_int32(delta);
    }
}

/// [`add`] where the analysis proved `index` in bounds.
///
/// # Safety
///
/// `index < xs.len()`.
pub unsafe fn add_unchecked<T: Element>(xs: &mut [T], index: usize, delta: i32) {
    // SAFETY: the caller guarantees `index` is in bounds.
    let slot = unsafe { xs.get_unchecked_mut(index) };
    *slot = slot.add_int32(delta);
}

/// The bits of a `number[]` element that is `undefined` (a hole) as the glue copies it in.
pub const HOLE: u64 = 0x7ff4_7472_696f_7869;

/// A value about to be stored in a `number[]`: [`HOLE`] becomes an ordinary NaN (what
/// `+undefined` is), so the glue only sees [`HOLE`] where the island didn't write. Every
/// store goes through it: `x * 1` and `x - 0` fold to `x`, and `-x` and `Math.abs` only
/// change the sign, so arithmetic can carry the pattern too. The selection is on the bits,
/// which the optimiser can't treat as an interchangeable NaN.
pub fn store_number(value: f64) -> f64 {
    let bits = value.to_bits();
    f64::from_bits(if bits == HOLE {
        f64::NAN.to_bits()
    } else {
        bits
    })
}

/// `xs.length` as a number.
#[expect(clippy::cast_precision_loss, reason = "lengths are below 2^32")]
pub fn len<T>(xs: &[T]) -> f64 {
    xs.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_reads() {
        let xs = [1.0, 2.0, 3.0];
        assert_eq!(get(&xs, 2.0), 3.0);
        assert_eq!(get(&xs, -0.0), 1.0);
        for index in [3.0, -1.0, 0.5, f64::NAN, f64::INFINITY, 1e300] {
            assert!(get(&xs, index).is_nan(), "{index}");
        }
        assert!(get_i32(&xs, -1).is_nan() && get_i32(&xs, 3).is_nan());
        assert_eq!(get_i32(&[U8Clamped(7)], 0), 7.0);
    }

    #[test]
    fn typed_writes_convert_and_ignore_out_of_bounds() {
        let mut bytes = [U8Clamped(0); 4];
        set(&mut bytes, 0.0, 2.5);
        set(&mut bytes, 1.0, 3.5);
        set(&mut bytes, 2.0, -7.0);
        set(&mut bytes, 3.0, 1e9);
        set(&mut bytes, 4.0, 1.0);
        set(&mut bytes, 0.5, 1.0);
        assert_eq!(
            bytes,
            [U8Clamped(2), U8Clamped(4), U8Clamped(0), U8Clamped(255)]
        );
        let mut ints = [0_i8; 2];
        set_i32(&mut ints, 0, 200.0);
        set_i32(&mut ints, -1, 1.0);
        assert_eq!(ints, [-56, 0]);
    }

    const DELTAS: [i32; 9] = [0, 1, -1, 2, 255, -256, 65_537, i32::MAX, i32::MIN];

    /// The integer shortcuts agree with the `f64` route they replace: `ToInt32` of the
    /// element as a number, and `from_f64(to_f64() + delta)`.
    fn agrees<T: Element + PartialEq + core::fmt::Debug>(values: &[T]) {
        for &value in values {
            assert_eq!(
                value.to_int32(),
                conv::to_int32(value.to_f64()),
                "{value:?}"
            );
            for delta in DELTAS {
                let slow = T::from_f64(value.to_f64() + f64::from(delta));
                let fast = value.add_int32(delta);
                assert!(
                    fast == slow || fast.to_f64().is_nan() && slow.to_f64().is_nan(),
                    "{value:?} + {delta}: {fast:?}, not {slow:?}"
                );
            }
        }
    }

    #[test]
    fn integer_shortcuts_match_number_arithmetic() {
        agrees(&[0_i8, 1, -1, i8::MAX, i8::MIN]);
        agrees(&[0_u8, 1, u8::MAX]);
        agrees(&[U8Clamped(0), U8Clamped(1), U8Clamped(254), U8Clamped(255)]);
        agrees(&[0_i16, -1, i16::MAX, i16::MIN]);
        agrees(&[0_u16, 1, u16::MAX]);
        agrees(&[0_i32, 1, -1, i32::MAX, i32::MIN]);
        agrees(&[0_u32, 1, u32::MAX, 1 << 31]);
        agrees(&[0.0_f32, -0.0, 0.5, f32::MAX, f32::NAN, 16_777_217.0]);
        agrees(&[
            0.0_f64,
            -0.0,
            0.5,
            9_007_199_254_740_993.0,
            f64::NAN,
            f64::INFINITY,
        ]);
    }

    #[test]
    fn integer_reads_and_updates_ignore_invalid_indices() {
        let mut bins = [u32::MAX, 7];
        add(&mut bins, 0.0, 1);
        add_i32(&mut bins, 1, -8);
        add(&mut bins, 0.5, 1);
        add_i32(&mut bins, -1, 1);
        add_usize(&mut bins, 2, 1);
        assert_eq!(bins, [0, u32::MAX]);
        assert_eq!(get_int32(&bins, 1.0), -1);
        assert_eq!(get_int32(&bins, -0.0), 0);
        for index in [2.0, -1.0, 0.5, f64::NAN] {
            assert_eq!(get_int32(&bins, index), 0, "{index}");
        }
        assert_eq!(get_int32_i32(&[f64::from_bits(HOLE)], 0), 0);
        assert_eq!(get_int32_usize(&[3.9_f64, -2.5], 1), -2);
    }
}
