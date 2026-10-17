//! WASM island `bitwise`, from `tests/islands/bitwise/kernel.ts` (hand-written in the shape
//! the Rust backend generates; see `docs/ABI.md`). Locals the analysis proves to be int32
//! are `i32`; `>>> 0` results are `u32`.

#![no_std]
#![allow(non_snake_case, clippy::all)]

use trioxide_rt::prelude::*;

fn fnv1a(bytes: &[u8]) -> u32 {
    // `0x811c9dc5` only ever reaches `^=` (ToInt32) or `>>> 0` (ToUint32), so its int32 bit
    // pattern stands for it.
    let mut hash: i32 = 0x811c9dc5_u32 as i32;
    let mut i = 0usize;
    while i < bytes.len() {
        // SAFETY: i < bytes.len().
        hash ^= i32::from(unsafe { *bytes.get_unchecked(i) });
        hash = math::imul_i32(hash, 0x01000193);
        i += 1;
    }
    hash as u32
}

fn crc32(bytes: &[u8]) -> u32 {
    let table = abi::alloc_array::<i32>(256);
    let mut n: i32 = 0;
    while n < 256 {
        let mut c: i32 = n;
        let mut k: i32 = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb88320_u32 as i32 ^ ops::ushr_i32(c, 1) as i32
            } else {
                ops::ushr_i32(c, 1) as i32
            };
            k += 1;
        }
        // SAFETY: 0 <= n < 256 == table.len().
        unsafe { *table.get_unchecked_mut(n as usize) = c };
        n += 1;
    }
    let mut crc: i32 = -1;
    let mut i = 0usize;
    while i < bytes.len() {
        // SAFETY: i < bytes.len(), and the table index is masked to 0..=255.
        unsafe {
            let index = (crc ^ i32::from(*bytes.get_unchecked(i))) & 0xff;
            crc = *table.get_unchecked(index as usize) ^ ops::ushr_i32(crc, 8) as i32;
        }
        i += 1;
    }
    (crc ^ -1) as u32
}

fn popcount(x: f64) -> i32 {
    let mut v: i32 = conv::to_int32(x);
    let mut count: i32 = 0;
    while v != 0 {
        v &= v.wrapping_sub(1);
        count += 1;
    }
    count
}

fn xorshift(seed: f64, out: &mut [i32]) -> i32 {
    let mut x: i32 = conv::to_int32(seed);
    if x == 0 {
        x = 1;
    }
    let mut i = 0usize;
    while i < out.len() {
        x ^= ops::shl_i32(x, 13);
        x ^= ops::shr_i32(x, 17);
        x ^= ops::shl_i32(x, 5);
        // SAFETY: i < out.len().
        unsafe { *out.get_unchecked_mut(i) = x };
        i += 1;
    }
    x
}

fn reverseBits(x: f64) -> u32 {
    let mut v: u32 = conv::to_uint32(x);
    let mut r: i32 = 0;
    let mut i: i32 = 0;
    while i < 32 {
        r = ops::shl_i32(r, 1) | (v & 1) as i32;
        v >>= 1;
        i += 1;
    }
    r as u32
}

fn packRgba(r: f64, g: f64, b: f64, a: f64) -> u32 {
    let a = ops::bit_and(a, 255.0);
    let b = ops::bit_and(b, 255.0);
    let g = ops::bit_and(g, 255.0);
    let r = ops::bit_and(r, 255.0);
    (ops::shl_i32(a, 24) | ops::shl_i32(b, 16) | ops::shl_i32(g, 8) | r) as u32
}

#[unsafe(export_name = "fnv1a")]
pub unsafe extern "C" fn __abi_fnv1a(bytes_ptr: u32, bytes_len: u32) -> u32 {
    // SAFETY: the glue passes a fresh copy.
    fnv1a(unsafe { abi::slice::<u8>(bytes_ptr, bytes_len) })
}

#[unsafe(export_name = "crc32")]
pub unsafe extern "C" fn __abi_crc32(bytes_ptr: u32, bytes_len: u32) -> u32 {
    // SAFETY: the glue passes a fresh copy.
    crc32(unsafe { abi::slice::<u8>(bytes_ptr, bytes_len) })
}

#[unsafe(export_name = "popcount")]
pub extern "C" fn __abi_popcount(x: f64) -> i32 {
    popcount(x)
}

#[unsafe(export_name = "xorshift")]
pub unsafe extern "C" fn __abi_xorshift(seed: f64, out_ptr: u32, out_len: u32) -> i32 {
    // SAFETY: the glue passes a fresh copy, aliasing nothing.
    xorshift(seed, unsafe { abi::slice_mut::<i32>(out_ptr, out_len) })
}

#[unsafe(export_name = "reverseBits")]
pub extern "C" fn __abi_reverseBits(x: f64) -> u32 {
    reverseBits(x)
}

#[unsafe(export_name = "packRgba")]
pub extern "C" fn __abi_packRgba(r: f64, g: f64, b: f64, a: f64) -> u32 {
    packRgba(r, g, b, a)
}
