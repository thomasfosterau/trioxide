//! Typed arrays and `number[]` inside an island.
//!
//! Arrays reach Rust as slices into linear memory (see [`abi`](crate::abi)). A typed array of
//! kind `K` is a slice of `K`'s [`Element`] type; a `number[]` is a `[f64]`.
//!
//! - **Reads**: [`get`] / [`get_i32`] return `NaN` for any index that is not an in-bounds
//!   integer, which is what `undefined` becomes in arithmetic. Reads the analysis proves in
//!   bounds use [`get_unchecked`] or plain `xs[i]`. A read whose `undefined` result could be
//!   observed other than through arithmetic (`=== undefined`, `??`) keeps the function in JS.
//! - **Typed array writes**: [`set`] / [`set_i32`] convert with the element type's rules and
//!   silently ignore invalid or out-of-bounds indices, as typed arrays do.
//! - **`number[]` writes** must be proven in bounds (an out-of-bounds write would grow the
//!   array, which v1 islands cannot express): use `xs[i] = v` or [`set_unchecked`].
//!   `number[]` never changes length inside an island.

use crate::{conv, fpu};

/// An element type of a typed array, with JavaScript's conversion to and from numbers.
pub trait Element: Copy + Default + 'static {
    /// The element as a number.
    fn to_f64(self) -> f64;
    /// The element a typed array stores for `value` (ECMA-262 `NumericToRawBytes`).
    fn from_f64(value: f64) -> Self;
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
                let bytes = conv::to_int32(value).to_le_bytes();
                let mut low = [0; size_of::<$ty>()];
                low.copy_from_slice(&bytes[..size_of::<$ty>()]);
                <$ty>::from_le_bytes(low)
            }
        }
    )*};
}

wrapping_element!(i8, u8, i16, u16);

impl Element for i32 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }

    fn from_f64(value: f64) -> Self {
        conv::to_int32(value)
    }
}

impl Element for u32 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }

    fn from_f64(value: f64) -> Self {
        conv::to_uint32(value)
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

/// `xs[index] = value` where the analysis proved `index` in bounds.
///
/// # Safety
///
/// `index < xs.len()`.
pub unsafe fn set_unchecked<T: Element>(xs: &mut [T], index: usize, value: f64) {
    // SAFETY: the caller guarantees `index` is in bounds.
    *unsafe { xs.get_unchecked_mut(index) } = T::from_f64(value);
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
}
