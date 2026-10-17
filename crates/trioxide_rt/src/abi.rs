//! The JS↔WASM boundary (`docs/ABI.md`).
//!
//! **Arena.** All boundary memory comes from one bump arena that starts at the linker's
//! `__heap_base` and grows linear memory on demand. The glue allocates argument copies with
//! `__trioxide_alloc`, calls the export, reads results and copies mutated arrays back, then
//! calls `__trioxide_reset`, which frees everything at once. Island code allocates its own
//! temporaries (`new Float64Array(n)`, returned structs) from the same arena, so nothing an
//! island allocates outlives the call. Memory is never shrunk.
//!
//! **Exports** (wasm32 only): `__trioxide_alloc(size, align) -> ptr` (0 when out of memory),
//! `__trioxide_reset()`, `__trioxide_abi_version() -> u32`, `__trioxide_trap_code() -> u32`
//! (why the last trap happened: [`TRAP_NONE`], [`TRAP_OUT_OF_MEMORY`] or
//! [`TRAP_INVALID_LENGTH`]) and `__trioxide_trap_value() -> f64` (the value it was about,
//! such as the invalid length, for the error message).
//!
//! **Arguments.** Pointers and lengths arrive as `u32`. [`slice`], [`slice_mut`],
//! [`struct_ref`] and [`struct_mut`] turn them into references.
//!
//! **Returns.** Arrays return the address of an [`ArrayRet`] made by [`return_array`];
//! structs return the address of a copy made by [`return_struct`].

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::array::Element;

/// No trap has been recorded.
pub const TRAP_NONE: u32 = 0;
/// The island ran out of linear memory allocating a temporary. The glue throws a
/// `RangeError`, as JavaScript does when an allocation is too large.
pub const TRAP_OUT_OF_MEMORY: u32 = 1;
/// A typed array constructor got an invalid length ([`new_array`]). The glue throws a
/// `RangeError`, as the constructor would.
pub const TRAP_INVALID_LENGTH: u32 = 2;

/// How an array return is described to the glue: where its elements are and how many.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrayRet {
    /// Address of the first element.
    pub ptr: u32,
    /// Number of elements.
    pub len: u32,
}

/// The next free arena address; 0 until first use.
static NEXT: AtomicUsize = AtomicUsize::new(0);
static TRAP_CODE: AtomicU32 = AtomicU32::new(TRAP_NONE);
static TRAP_VALUE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "wasm32")]
mod memory {
    const PAGE: usize = 65_536;

    unsafe extern "C" {
        static __heap_base: u8;
    }

    pub fn base() -> usize {
        core::ptr::addr_of!(__heap_base) as usize
    }

    /// Grows linear memory so that addresses below `end` are valid.
    pub fn reserve(end: usize) -> bool {
        let size = core::arch::wasm32::memory_size(0) * PAGE;
        end <= size || core::arch::wasm32::memory_grow(0, (end - size).div_ceil(PAGE)) != usize::MAX
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod memory {
    //! A fixed host-side arena so the helpers can be unit tested natively.

    use core::cell::UnsafeCell;

    const SIZE: usize = 1 << 20;

    #[repr(C, align(16))]
    struct Arena(UnsafeCell<[u8; SIZE]>);

    // SAFETY: only reached through the single-threaded arena API; host tests that use it
    // serialise themselves.
    unsafe impl Sync for Arena {}

    static ARENA: Arena = Arena(UnsafeCell::new([0; SIZE]));

    pub fn base() -> usize {
        ARENA.0.get() as usize
    }

    pub fn reserve(end: usize) -> bool {
        end <= base() + SIZE
    }
}

fn next() -> usize {
    match NEXT.load(Ordering::Relaxed) {
        0 => memory::base(),
        next => next,
    }
}

/// Allocates `size` bytes aligned to `align` (a power of two) from the arena. Returns null
/// when linear memory cannot grow enough or `align` is not a power of two.
pub fn alloc(size: usize, align: usize) -> *mut u8 {
    if !align.is_power_of_two() {
        return core::ptr::null_mut();
    }
    let Some(start) = next().checked_next_multiple_of(align) else {
        return core::ptr::null_mut();
    };
    let Some(end) = start.checked_add(size) else {
        return core::ptr::null_mut();
    };
    if !memory::reserve(end) {
        return core::ptr::null_mut();
    }
    NEXT.store(end, Ordering::Relaxed);
    start as *mut u8
}

/// Frees every arena allocation at once and clears the trap code.
pub fn reset() {
    NEXT.store(memory::base(), Ordering::Relaxed);
    TRAP_CODE.store(TRAP_NONE, Ordering::Relaxed);
}

/// Records why the island is about to trap, then traps.
pub fn trap(code: u32) -> ! {
    trap_with(code, f64::NAN)
}

/// Records why the island is about to trap and the value concerned, then traps.
pub fn trap_with(code: u32, value: f64) -> ! {
    TRAP_CODE.store(code, Ordering::Relaxed);
    TRAP_VALUE.store(value.to_bits(), Ordering::Relaxed);
    #[cfg(target_arch = "wasm32")]
    core::arch::wasm32::unreachable();
    #[cfg(not(target_arch = "wasm32"))]
    panic!("island trap {code}");
}

/// The trap code recorded by the last [`trap`].
pub fn trap_code() -> u32 {
    TRAP_CODE.load(Ordering::Relaxed)
}

/// The value recorded by the last [`trap_with`] (NaN after [`trap`]).
pub fn trap_value() -> f64 {
    f64::from_bits(TRAP_VALUE.load(Ordering::Relaxed))
}

/// A new zero-filled array of `len` elements in the arena, valid until the glue resets it:
/// `new Float64Array(len)` (or any typed array) inside an island. Traps with
/// [`TRAP_OUT_OF_MEMORY`] if memory runs out.
pub fn alloc_array<T: Element>(len: usize) -> &'static mut [T] {
    if len == 0 {
        return &mut [];
    }
    let Some(size) = len.checked_mul(size_of::<T>()) else {
        trap(TRAP_OUT_OF_MEMORY)
    };
    let ptr = alloc(size, align_of::<T>()).cast::<T>();
    if ptr.is_null() {
        trap(TRAP_OUT_OF_MEMORY);
    }
    // SAFETY: `ptr` is aligned and owns `size` fresh bytes until the next reset; every
    // `Element` is valid as all-zero bytes (0, +0.0).
    unsafe {
        ptr.write_bytes(0, len);
        core::slice::from_raw_parts_mut(ptr, len)
    }
}

/// `new Float64Array(length)` (or any typed array kind) inside an island: `length` goes
/// through `ToIndex` (truncated, NaN → 0), and lengths that are negative or beyond 2^53 - 1
/// trap with [`TRAP_INVALID_LENGTH`]. Zero-filled, from the arena like [`alloc_array`].
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "a non-negative integer; lengths beyond usize can't be allocated anyway"
)]
pub fn new_array<T: Element>(length: f64) -> &'static mut [T] {
    let argument = length;
    let length = if length.is_nan() {
        0.0
    } else {
        crate::math::trunc(length)
    };
    if !(0.0..=9_007_199_254_740_991.0).contains(&length) {
        trap_with(TRAP_INVALID_LENGTH, argument);
    }
    if length > usize::MAX as f64 {
        trap(TRAP_OUT_OF_MEMORY);
    }
    alloc_array(length as usize)
}

/// The array argument at `ptr`/`len` (an element count).
///
/// # Safety
///
/// `ptr`/`len` came from the glue for an array parameter of element type `T`, and no
/// `&mut` to the same memory exists (the glue never passes overlapping arrays).
pub unsafe fn slice<'a, T: Element>(ptr: u32, len: u32) -> &'a [T] {
    if len == 0 {
        return &[];
    }
    // SAFETY: the glue copied `len` elements of `T` to the aligned address `ptr`.
    unsafe { core::slice::from_raw_parts(ptr as usize as *const T, len as usize) }
}

/// The mutable array argument at `ptr`/`len`.
///
/// # Safety
///
/// As for [`slice`], and no other reference to the same memory exists.
pub unsafe fn slice_mut<'a, T: Element>(ptr: u32, len: u32) -> &'a mut [T] {
    if len == 0 {
        return &mut [];
    }
    // SAFETY: as for `slice`; the glue passes each array exactly once.
    unsafe { core::slice::from_raw_parts_mut(ptr as usize as *mut T, len as usize) }
}

/// The struct argument at `ptr`.
///
/// # Safety
///
/// `ptr` came from the glue for a struct parameter whose layout `T` mirrors.
pub unsafe fn struct_ref<'a, T: Copy>(ptr: u32) -> &'a T {
    // SAFETY: the glue wrote a `T` at the aligned address `ptr`.
    unsafe { &*(ptr as usize as *const T) }
}

/// The mutable struct argument at `ptr`.
///
/// # Safety
///
/// As for [`struct_ref`], and no other reference to the same memory exists.
pub unsafe fn struct_mut<'a, T: Copy>(ptr: u32) -> &'a mut T {
    // SAFETY: as for `struct_ref`.
    unsafe { &mut *(ptr as usize as *mut T) }
}

/// Returns an array: records where `xs` is and returns the address of that [`ArrayRet`]. The
/// glue copies the elements into a new JavaScript array, so `xs` may be an arena array, an
/// argument, or any other memory that lives until the reset.
#[expect(
    clippy::cast_possible_truncation,
    reason = "addresses and lengths fit u32 on wasm32"
)]
pub fn return_array<T: Element>(xs: &[T]) -> u32 {
    return_struct(ArrayRet {
        ptr: xs.as_ptr() as usize as u32,
        len: xs.len() as u32,
    })
}

/// Returns a struct: copies `value` into the arena and returns its address.
#[expect(
    clippy::cast_possible_truncation,
    reason = "addresses fit u32 on wasm32"
)]
pub fn return_struct<T: Copy>(value: T) -> u32 {
    let ptr = alloc(size_of::<T>(), align_of::<T>()).cast::<T>();
    if ptr.is_null() {
        trap(TRAP_OUT_OF_MEMORY);
    }
    // SAFETY: `ptr` is fresh, aligned and large enough.
    unsafe { ptr.write(value) };
    ptr as usize as u32
}

#[cfg(target_arch = "wasm32")]
mod exports {
    #[unsafe(no_mangle)]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "addresses fit u32 on wasm32"
    )]
    extern "C" fn __trioxide_alloc(size: u32, align: u32) -> u32 {
        super::alloc(size as usize, align as usize) as usize as u32
    }

    #[unsafe(no_mangle)]
    extern "C" fn __trioxide_reset() {
        super::reset();
    }

    #[unsafe(no_mangle)]
    extern "C" fn __trioxide_abi_version() -> u32 {
        crate::ABI_VERSION
    }

    #[unsafe(no_mangle)]
    extern "C" fn __trioxide_trap_code() -> u32 {
        super::trap_code()
    }

    #[unsafe(no_mangle)]
    extern "C" fn __trioxide_trap_value() -> f64 {
        super::trap_value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test, because the host arena is a process-wide static.
    #[test]
    fn arena() {
        reset();
        let a = alloc(3, 1);
        let b = alloc(8, 8);
        assert!(!a.is_null() && !b.is_null());
        assert_eq!(b as usize % 8, 0);
        assert!(b as usize >= a as usize + 3);
        assert!(alloc(1, 3).is_null());
        assert!(alloc(usize::MAX, 1).is_null());

        let xs = alloc_array::<f64>(4);
        assert_eq!(xs, [0.0; 4]);
        xs[1] = 2.5;
        assert_ne!(return_struct(ArrayRet { ptr: 1, len: 4 }), 0);

        reset();
        assert_eq!(alloc(3, 1), a, "reset frees everything");
        reset();
    }
}
