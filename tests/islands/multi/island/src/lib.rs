//! WASM island `multi`, from `tests/islands/multi/kernel.ts` (hand-written in the shape the
//! Rust backend generates; see `docs/ABI.md`). Exports call each other through the internal
//! functions, never through the ABI shims.

#![no_std]
#![allow(non_snake_case, clippy::all)]

use trioxide_rt::prelude::*;

fn at(n: f64, row: f64, col: f64) -> f64 {
    row * n + col
}

fn swapRows(a: &mut [f64], n: f64, r1: f64, r2: f64) {
    let mut k = 0.0;
    while k < n {
        let t = array::get(a, at(n, r1, k));
        array::set(a, at(n, r1, k), array::get(a, at(n, r2, k)));
        array::set(a, at(n, r2, k), t);
        k += 1.0;
    }
}

fn identity(n: f64) -> &'static mut [f64] {
    let m = abi::new_array::<f64>(n * n);
    let mut i = 0.0;
    while i < n {
        array::set(m, at(n, i, i), 1.0);
        i += 1.0;
    }
    m
}

fn multiply(a: &[f64], b: &[f64], n: f64) -> &'static mut [f64] {
    let out = abi::new_array::<f64>(n * n);
    let mut i = 0.0;
    while i < n {
        let mut j = 0.0;
        while j < n {
            let mut s = 0.0;
            let mut k = 0.0;
            while k < n {
                s += array::get(a, at(n, i, k)) * array::get(b, at(n, k, j));
                k += 1.0;
            }
            array::set(out, at(n, i, j), s);
            j += 1.0;
        }
        i += 1.0;
    }
    out
}

fn power(m: &[f64], n: f64, mut k: f64) -> &'static [f64] {
    let mut result: &'static [f64] = identity(n);
    let mut base: &[f64] = m;
    while k > 0.0 {
        if ops::rem(k, 2.0) == 1.0 {
            result = multiply(result, base, n);
        }
        base = multiply(base, base, n);
        k = math::floor(k / 2.0);
    }
    result
}

fn trace(m: &[f64], n: f64) -> f64 {
    let mut t = 0.0;
    let mut i = 0.0;
    while i < n {
        t += array::get(m, at(n, i, i));
        i += 1.0;
    }
    t
}

fn determinant(m: &[f64], n: f64) -> f64 {
    let a = abi::alloc_array::<f64>(m.len());
    let mut i = 0usize;
    while i < m.len() {
        // SAFETY: i < m.len() == a.len().
        unsafe { array::set_unchecked(a, i, array::get_unchecked(m, i)) };
        i += 1;
    }
    let mut det = 1.0;
    let mut col = 0.0;
    while col < n {
        let mut pivot = col;
        let mut row = col + 1.0;
        while row < n {
            if math::abs(array::get(a, at(n, row, col))) > math::abs(array::get(a, at(n, pivot, col))) {
                pivot = row;
            }
            row += 1.0;
        }
        if array::get(a, at(n, pivot, col)) == 0.0 {
            return 0.0;
        }
        if pivot != col {
            swapRows(a, n, pivot, col);
            det = -det;
        }
        det *= array::get(a, at(n, col, col));
        let mut row = col + 1.0;
        while row < n {
            let f = array::get(a, at(n, row, col)) / array::get(a, at(n, col, col));
            let mut k = col;
            while k < n {
                let value = array::get(a, at(n, row, k)) - f * array::get(a, at(n, col, k));
                array::set(a, at(n, row, k), value);
                k += 1.0;
            }
            row += 1.0;
        }
        col += 1.0;
    }
    det
}

fn axpy(y: &mut [f64], x: &[f64], alpha: f64) {
    let mut i = 0usize;
    while i < y.len() {
        // SAFETY: i < y.len(); `x` may be shorter.
        unsafe {
            let value = array::get_unchecked(y, i) + alpha * array::get_usize(x, i);
            array::set_unchecked(y, i, value);
        }
        i += 1;
    }
}

#[unsafe(export_name = "identity")]
pub extern "C" fn __abi_identity(n: f64) -> u32 {
    abi::return_array(identity(n))
}

#[unsafe(export_name = "multiply")]
pub unsafe extern "C" fn __abi_multiply(a_ptr: u32, a_len: u32, b_ptr: u32, b_len: u32, n: f64) -> u32 {
    // SAFETY: the glue passes fresh copies.
    let (a, b) = unsafe { (abi::slice::<f64>(a_ptr, a_len), abi::slice::<f64>(b_ptr, b_len)) };
    abi::return_array(multiply(a, b, n))
}

#[unsafe(export_name = "power")]
pub unsafe extern "C" fn __abi_power(m_ptr: u32, m_len: u32, n: f64, k: f64) -> u32 {
    // SAFETY: the glue passes a fresh copy.
    abi::return_array(power(unsafe { abi::slice::<f64>(m_ptr, m_len) }, n, k))
}

#[unsafe(export_name = "trace")]
pub unsafe extern "C" fn __abi_trace(m_ptr: u32, m_len: u32, n: f64) -> f64 {
    // SAFETY: the glue passes a fresh copy.
    trace(unsafe { abi::slice::<f64>(m_ptr, m_len) }, n)
}

#[unsafe(export_name = "determinant")]
pub unsafe extern "C" fn __abi_determinant(m_ptr: u32, m_len: u32, n: f64) -> f64 {
    // SAFETY: the glue passes a fresh copy.
    determinant(unsafe { abi::slice::<f64>(m_ptr, m_len) }, n)
}

#[unsafe(export_name = "axpy")]
pub unsafe extern "C" fn __abi_axpy(y_ptr: u32, y_len: u32, x_ptr: u32, x_len: u32, alpha: f64) {
    // SAFETY: the glue passes fresh copies and calls the JS version when they overlap.
    let (y, x) = unsafe { (abi::slice_mut::<f64>(y_ptr, y_len), abi::slice::<f64>(x_ptr, x_len)) };
    axpy(y, x, alpha);
}
