//! WASM island `nbody`, from `tests/islands/nbody/kernel.ts` (hand-written in the shape the
//! Rust backend generates; see `docs/ABI.md`).

#![no_std]
#![allow(non_snake_case, clippy::all)]

use trioxide_rt::prelude::*;

const STRIDE: f64 = 7.0;

/// `bodies[index]` (a read the analysis can't prove in bounds).
fn at(bodies: &[f64], index: f64) -> f64 {
    array::get(bodies, index)
}

fn pull(dx: f64, dy: f64, dz: f64, dt: f64) -> f64 {
    let d2 = dx * dx + dy * dy + dz * dz;
    dt / (d2 * math::sqrt(d2))
}

fn advance(bodies: &mut [f64], dt: f64, steps: f64) {
    let n = array::len(bodies) / STRIDE;
    let mut step = 0.0;
    while step < steps {
        let mut i = 0.0;
        while i < n {
            let bi = i * STRIDE;
            let mut j = i + 1.0;
            while j < n {
                let bj = j * STRIDE;
                let dx = at(bodies, bi) - at(bodies, bj);
                let dy = at(bodies, bi + 1.0) - at(bodies, bj + 1.0);
                let dz = at(bodies, bi + 2.0) - at(bodies, bj + 2.0);
                let mag = pull(dx, dy, dz, dt);
                let mi = at(bodies, bi + 6.0) * mag;
                let mj = at(bodies, bj + 6.0) * mag;
                array::set(bodies, bi + 3.0, at(bodies, bi + 3.0) - dx * mj);
                array::set(bodies, bi + 4.0, at(bodies, bi + 4.0) - dy * mj);
                array::set(bodies, bi + 5.0, at(bodies, bi + 5.0) - dz * mj);
                array::set(bodies, bj + 3.0, at(bodies, bj + 3.0) + dx * mi);
                array::set(bodies, bj + 4.0, at(bodies, bj + 4.0) + dy * mi);
                array::set(bodies, bj + 5.0, at(bodies, bj + 5.0) + dz * mi);
                j += 1.0;
            }
            i += 1.0;
        }
        let mut i = 0.0;
        while i < n {
            let b = i * STRIDE;
            array::set(bodies, b, at(bodies, b) + dt * at(bodies, b + 3.0));
            array::set(bodies, b + 1.0, at(bodies, b + 1.0) + dt * at(bodies, b + 4.0));
            array::set(bodies, b + 2.0, at(bodies, b + 2.0) + dt * at(bodies, b + 5.0));
            i += 1.0;
        }
        step += 1.0;
    }
}

fn energy(bodies: &[f64]) -> f64 {
    let n = array::len(bodies) / STRIDE;
    let mut e = 0.0;
    let mut i = 0.0;
    while i < n {
        let bi = i * STRIDE;
        let m = at(bodies, bi + 6.0);
        e += 0.5
            * m
            * (math::pow(at(bodies, bi + 3.0), 2.0)
                + math::pow(at(bodies, bi + 4.0), 2.0)
                + math::pow(at(bodies, bi + 5.0), 2.0));
        let mut j = i + 1.0;
        while j < n {
            let bj = j * STRIDE;
            e -= (m * at(bodies, bj + 6.0))
                / distance(
                    at(bodies, bi) - at(bodies, bj),
                    at(bodies, bi + 1.0) - at(bodies, bj + 1.0),
                    at(bodies, bi + 2.0) - at(bodies, bj + 2.0),
                );
            j += 1.0;
        }
        i += 1.0;
    }
    e
}

fn distance(dx: f64, dy: f64, dz: f64) -> f64 {
    math::sqrt(dx * dx + dy * dy + dz * dz)
}

#[unsafe(export_name = "advance")]
pub unsafe extern "C" fn __abi_advance(bodies_ptr: u32, bodies_len: u32, dt: f64, steps: f64) {
    // SAFETY: the glue passes a fresh copy of a Float64Array, aliasing nothing.
    let bodies = unsafe { abi::slice_mut::<f64>(bodies_ptr, bodies_len) };
    advance(bodies, dt, steps);
}

#[unsafe(export_name = "energy")]
pub unsafe extern "C" fn __abi_energy(bodies_ptr: u32, bodies_len: u32) -> f64 {
    // SAFETY: the glue passes a fresh copy of a Float64Array.
    let bodies = unsafe { abi::slice::<f64>(bodies_ptr, bodies_len) };
    energy(bodies)
}

#[unsafe(export_name = "distance")]
pub extern "C" fn __abi_distance(dx: f64, dy: f64, dz: f64) -> f64 {
    distance(dx, dy, dz)
}
