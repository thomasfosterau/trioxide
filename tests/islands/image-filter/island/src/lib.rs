//! WASM island `image-filter`, from `tests/islands/image-filter/kernel.ts` (hand-written in
//! the shape the Rust backend generates; see `docs/ABI.md`).

#![no_std]
#![allow(non_snake_case, clippy::all)]

use trioxide_rt::prelude::*;

fn brighten(pixels: &mut [U8Clamped], factor: f64) {
    let mut i = 0usize;
    while i < pixels.len() {
        if (i as i32 & 3) != 3 {
            // SAFETY: i < pixels.len().
            unsafe {
                let value = array::get_unchecked(pixels, i) * factor;
                array::set_unchecked(pixels, i, value);
            }
        }
        i += 1;
    }
}

fn grayscale(pixels: &mut [U8Clamped]) {
    let mut i = 0usize;
    while i + 3 < pixels.len() {
        // SAFETY: i + 3 < pixels.len().
        unsafe {
            let luma = 0.299 * array::get_unchecked(pixels, i)
                + 0.587 * array::get_unchecked(pixels, i + 1)
                + 0.114 * array::get_unchecked(pixels, i + 2);
            array::set_unchecked(pixels, i, luma);
            array::set_unchecked(pixels, i + 1, luma);
            array::set_unchecked(pixels, i + 2, luma);
        }
        i += 4;
    }
}

fn boxBlur(src: &[U8Clamped], width: f64, height: f64) -> &'static mut [U8Clamped] {
    let dst = abi::alloc_array::<U8Clamped>(src.len());
    let mut y = 0.0;
    while y < height {
        let mut x = 0.0;
        while x < width {
            let mut c = 0.0;
            while c < 4.0 {
                let mut sum = 0.0;
                let mut count = 0.0;
                let mut dy = -1.0;
                while dy <= 1.0 {
                    let mut dx = -1.0;
                    while dx <= 1.0 {
                        let nx = x + dx;
                        let ny = y + dy;
                        if nx >= 0.0 && nx < width && ny >= 0.0 && ny < height {
                            sum += array::get(src, (ny * width + nx) * 4.0 + c);
                            count += 1.0;
                        }
                        dx += 1.0;
                    }
                    dy += 1.0;
                }
                array::set(dst, (y * width + x) * 4.0 + c, sum / count);
                c += 1.0;
            }
            x += 1.0;
        }
        y += 1.0;
    }
    dst
}

fn histogram(pixels: &[U8Clamped], bins: &mut [u32]) {
    let mut i = 0usize;
    while i < pixels.len() {
        if (i as i32 & 3) != 3 {
            // SAFETY: i < pixels.len().
            let bin = ops::shr(unsafe { array::get_unchecked(pixels, i) }, 4.0);
            array::set_i32(bins, bin, array::get_i32(bins, bin) + 1.0);
        }
        i += 1;
    }
}

#[unsafe(export_name = "brighten")]
pub unsafe extern "C" fn __abi_brighten(pixels_ptr: u32, pixels_len: u32, factor: f64) {
    // SAFETY: the glue passes a fresh copy, aliasing nothing.
    let pixels = unsafe { abi::slice_mut::<U8Clamped>(pixels_ptr, pixels_len) };
    brighten(pixels, factor);
}

#[unsafe(export_name = "grayscale")]
pub unsafe extern "C" fn __abi_grayscale(pixels_ptr: u32, pixels_len: u32) {
    // SAFETY: the glue passes a fresh copy, aliasing nothing.
    let pixels = unsafe { abi::slice_mut::<U8Clamped>(pixels_ptr, pixels_len) };
    grayscale(pixels);
}

#[unsafe(export_name = "boxBlur")]
pub unsafe extern "C" fn __abi_boxBlur(src_ptr: u32, src_len: u32, width: f64, height: f64) -> u32 {
    // SAFETY: the glue passes a fresh copy.
    let src = unsafe { abi::slice::<U8Clamped>(src_ptr, src_len) };
    abi::return_array(boxBlur(src, width, height))
}

#[unsafe(export_name = "histogram")]
pub unsafe extern "C" fn __abi_histogram(pixels_ptr: u32, pixels_len: u32, bins_ptr: u32, bins_len: u32) {
    // SAFETY: the glue passes fresh copies and checks they don't overlap in JS.
    let pixels = unsafe { abi::slice::<U8Clamped>(pixels_ptr, pixels_len) };
    let bins = unsafe { abi::slice_mut::<u32>(bins_ptr, bins_len) };
    histogram(pixels, bins);
}
