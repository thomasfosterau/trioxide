//! WASM island `array-reduce`, from `tests/islands/array-reduce/kernel.ts` (hand-written in
//! the shape the Rust backend generates; see `docs/ABI.md`).

#![no_std]
#![allow(non_snake_case, clippy::all)]

use trioxide_rt::prelude::*;

/// `{ min, max, mean }`, the object literal `stats` returns (`structs[0]` in the spec).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Stats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
}

const _: () = assert!(size_of::<Stats>() == 24 && align_of::<Stats>() == 8);

fn sum(values: &[f64]) -> f64 {
    let mut total = 0.0;
    let mut i = 0usize;
    while i < values.len() {
        // SAFETY: i < values.len().
        total += unsafe { array::get_unchecked(values, i) };
        i += 1;
    }
    total
}

fn stats(values: &[f64]) -> Stats {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut total = 0.0;
    for &value in values {
        min = math::min(min, value);
        max = math::max(max, value);
        total += value;
    }
    Stats {
        min,
        max,
        mean: total / array::len(values),
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut total = 0.0;
    let mut i = 0usize;
    while i < a.len() {
        // SAFETY: i < a.len(); `b` is not known to be as long.
        total += unsafe { array::get_unchecked(a, i) } * array::get_usize(b, i);
        i += 1;
    }
    total
}

fn normalize(values: &mut [f64]) {
    let total = sum(values);
    let mut i = 0usize;
    while i < values.len() {
        // SAFETY: i < values.len().
        unsafe {
            let value = array::get_unchecked(values, i) / total;
            array::set_unchecked(values, i, value);
        }
        i += 1;
    }
}

fn prefixSums(values: &[f64]) -> &'static mut [f64] {
    let out = abi::alloc_array::<f64>(values.len());
    let mut total = 0.0;
    let mut i = 0usize;
    while i < values.len() {
        // SAFETY: i < values.len() == out.len().
        unsafe {
            total += array::get_unchecked(values, i);
            array::set_unchecked(out, i, total);
        }
        i += 1;
    }
    out
}

fn weightedMean(values: &[f64], weights: &[f64]) -> f64 {
    let mut total = 0.0;
    let mut weight = 0.0;
    let mut i = 0usize;
    while i < values.len() {
        // SAFETY: i < values.len().
        total += unsafe { array::get_unchecked(values, i) } * array::get_usize(weights, i);
        weight += array::get_usize(weights, i);
        i += 1;
    }
    total / weight
}

#[unsafe(export_name = "sum")]
pub unsafe extern "C" fn __abi_sum(values_ptr: u32, values_len: u32) -> f64 {
    // SAFETY: the glue passes a fresh copy.
    sum(unsafe { abi::slice::<f64>(values_ptr, values_len) })
}

#[unsafe(export_name = "stats")]
pub unsafe extern "C" fn __abi_stats(values_ptr: u32, values_len: u32) -> u32 {
    // SAFETY: the glue passes a fresh copy.
    abi::return_struct(stats(unsafe { abi::slice::<f64>(values_ptr, values_len) }))
}

#[unsafe(export_name = "dot")]
pub unsafe extern "C" fn __abi_dot(a_ptr: u32, a_len: u32, b_ptr: u32, b_len: u32) -> f64 {
    // SAFETY: the glue passes fresh copies.
    unsafe { dot(abi::slice::<f64>(a_ptr, a_len), abi::slice::<f64>(b_ptr, b_len)) }
}

#[unsafe(export_name = "normalize")]
pub unsafe extern "C" fn __abi_normalize(values_ptr: u32, values_len: u32) {
    // SAFETY: the glue passes a fresh copy, aliasing nothing.
    normalize(unsafe { abi::slice_mut::<f64>(values_ptr, values_len) });
}

#[unsafe(export_name = "prefixSums")]
pub unsafe extern "C" fn __abi_prefixSums(values_ptr: u32, values_len: u32) -> u32 {
    // SAFETY: the glue passes a fresh copy.
    abi::return_array(prefixSums(unsafe { abi::slice::<f64>(values_ptr, values_len) }))
}

#[unsafe(export_name = "weightedMean")]
pub unsafe extern "C" fn __abi_weightedMean(
    values_ptr: u32,
    values_len: u32,
    weights_ptr: u32,
    weights_len: u32,
) -> f64 {
    // SAFETY: the glue passes fresh copies.
    unsafe {
        weightedMean(
            abi::slice::<f64>(values_ptr, values_len),
            abi::slice::<f64>(weights_ptr, weights_len),
        )
    }
}
