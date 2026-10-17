//! WASM island `errors`, from `tests/islands/errors/kernel.ts` (hand-written in the shape the
//! Rust backend generates; see `docs/ABI.md`).

#![no_std]
#![allow(non_snake_case, clippy::all)]

use trioxide_rt::prelude::*;

fn filled(length: f64, value: f64) -> &'static mut [f64] {
    let out = abi::new_array::<f64>(length);
    let mut i = 0usize;
    while i < out.len() {
        // SAFETY: i < out.len().
        unsafe { array::set_unchecked(out, i, value) };
        i += 1;
    }
    out
}

fn total(length: f64) -> f64 {
    let scratch = abi::new_array::<f64>(length);
    let mut sum = 0.0;
    let mut i = 0usize;
    while i < scratch.len() {
        // SAFETY: i < scratch.len().
        unsafe {
            array::set_unchecked(scratch, i, i as f64);
            sum += array::get_unchecked(scratch, i);
        }
        i += 1;
    }
    sum
}

#[unsafe(export_name = "filled")]
pub extern "C" fn __abi_filled(length: f64, value: f64) -> u32 {
    abi::return_array(filled(length, value))
}

#[unsafe(export_name = "total")]
pub extern "C" fn __abi_total(length: f64) -> f64 {
    total(length)
}
