//! WASM island `math-edge`, from `tests/islands/math-edge/kernel.ts` (hand-written in the
//! shape the Rust backend generates; see `docs/ABI.md`).

#![no_std]
#![allow(non_snake_case, clippy::all)]

use trioxide_rt::prelude::*;

fn round(x: f64) -> f64 {
    math::round(x)
}

fn sign(x: f64) -> f64 {
    math::sign(x)
}

fn trunc(x: f64) -> f64 {
    math::trunc(x)
}

fn fround(x: f64) -> f64 {
    math::fround(x)
}

fn max(a: f64, b: f64) -> f64 {
    math::max(a, b)
}

fn min3(a: f64, b: f64, c: f64) -> f64 {
    math::min(math::min(a, b), c)
}

fn hypot(a: f64, b: f64, c: f64) -> f64 {
    math::hypot3(a, b, c)
}

fn rem(a: f64, b: f64) -> f64 {
    ops::rem(a, b)
}

fn clz32(x: f64) -> i32 {
    math::clz32(x) as i32
}

fn imul(a: f64, b: f64) -> i32 {
    math::imul(a, b)
}

fn toInt32(x: f64) -> i32 {
    conv::to_int32(x)
}

fn toUint32(x: f64) -> u32 {
    conv::to_uint32(x)
}

fn mix(a: f64, b: f64) -> i32 {
    ops::shl(a, 3.0) ^ ops::shr(b, 1.0) ^ ops::ushr(f64::from(ops::bit_not(a)), 28.0) as i32
}

fn idiv(a: i32, b: i32) -> i32 {
    ops::div_trunc_i32(a, b)
}

fn irem(a: i32, b: i32) -> f64 {
    ops::rem_i32(a, b)
}

fn wrapAdd(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

fn isPositive(x: f64) -> bool {
    x > 0.0
}

fn notLess(a: f64, b: f64) -> bool {
    !(a < b)
}

fn choose(flag: bool, a: f64, b: f64) -> f64 {
    if flag { a } else { b }
}

fn truthy(x: f64) -> bool {
    conv::to_boolean(x)
}

fn clampByte(x: f64) -> f64 {
    let out = abi::alloc_array::<U8Clamped>(1);
    array::set(out, 0.0, x);
    array::get(out, 0.0)
}

#[unsafe(export_name = "round")]
pub extern "C" fn __abi_round(x: f64) -> f64 {
    round(x)
}

#[unsafe(export_name = "sign")]
pub extern "C" fn __abi_sign(x: f64) -> f64 {
    sign(x)
}

#[unsafe(export_name = "trunc")]
pub extern "C" fn __abi_trunc(x: f64) -> f64 {
    trunc(x)
}

#[unsafe(export_name = "fround")]
pub extern "C" fn __abi_fround(x: f64) -> f64 {
    fround(x)
}

#[unsafe(export_name = "max")]
pub extern "C" fn __abi_max(a: f64, b: f64) -> f64 {
    max(a, b)
}

#[unsafe(export_name = "min3")]
pub extern "C" fn __abi_min3(a: f64, b: f64, c: f64) -> f64 {
    min3(a, b, c)
}

#[unsafe(export_name = "hypot")]
pub extern "C" fn __abi_hypot(a: f64, b: f64, c: f64) -> f64 {
    hypot(a, b, c)
}

#[unsafe(export_name = "rem")]
pub extern "C" fn __abi_rem(a: f64, b: f64) -> f64 {
    rem(a, b)
}

#[unsafe(export_name = "clz32")]
pub extern "C" fn __abi_clz32(x: f64) -> i32 {
    clz32(x)
}

#[unsafe(export_name = "imul")]
pub extern "C" fn __abi_imul(a: f64, b: f64) -> i32 {
    imul(a, b)
}

#[unsafe(export_name = "toInt32")]
pub extern "C" fn __abi_toInt32(x: f64) -> i32 {
    toInt32(x)
}

#[unsafe(export_name = "toUint32")]
pub extern "C" fn __abi_toUint32(x: f64) -> u32 {
    toUint32(x)
}

#[unsafe(export_name = "mix")]
pub extern "C" fn __abi_mix(a: f64, b: f64) -> i32 {
    mix(a, b)
}

#[unsafe(export_name = "idiv")]
pub extern "C" fn __abi_idiv(a: i32, b: i32) -> i32 {
    idiv(a, b)
}

#[unsafe(export_name = "irem")]
pub extern "C" fn __abi_irem(a: i32, b: i32) -> f64 {
    irem(a, b)
}

#[unsafe(export_name = "wrapAdd")]
pub extern "C" fn __abi_wrapAdd(a: u32, b: u32) -> u32 {
    wrapAdd(a, b)
}

#[unsafe(export_name = "isPositive")]
pub extern "C" fn __abi_isPositive(x: f64) -> i32 {
    i32::from(isPositive(x))
}

#[unsafe(export_name = "notLess")]
pub extern "C" fn __abi_notLess(a: f64, b: f64) -> i32 {
    i32::from(notLess(a, b))
}

#[unsafe(export_name = "choose")]
pub extern "C" fn __abi_choose(flag: i32, a: f64, b: f64) -> f64 {
    choose(flag != 0, a, b)
}

#[unsafe(export_name = "truthy")]
pub extern "C" fn __abi_truthy(x: f64) -> i32 {
    i32::from(truthy(x))
}

#[unsafe(export_name = "clampByte")]
pub extern "C" fn __abi_clampByte(x: f64) -> f64 {
    clampByte(x)
}
