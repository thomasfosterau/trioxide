//! WASM island `particles`, from `tests/islands/particles/kernel.ts` (hand-written in the
//! shape the Rust backend generates; see `docs/ABI.md`).

#![no_std]
#![allow(non_snake_case, clippy::all)]

use trioxide_rt::prelude::*;

/// The internal class `Vec2`. It never crosses the boundary, so it has no spec entry and
/// its layout is Rust's business.
#[derive(Clone, Copy)]
struct Vec2 {
    x: f64,
    y: f64,
}

impl Vec2 {
    fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn add(self, other: Vec2) -> Vec2 {
        Vec2::new(self.x + other.x, self.y + other.y)
    }

    fn scale(self, k: f64) -> Vec2 {
        Vec2::new(self.x * k, self.y * k)
    }

    fn length(self) -> f64 {
        math::hypot2(self.x, self.y)
    }
}

/// `Body` (`structs[0]` in the spec).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Body {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub mass: f64,
    pub layer: i32,
    pub pinned: bool,
}

const _: () = {
    assert!(size_of::<Body>() == 48 && align_of::<Body>() == 8);
    assert!(core::mem::offset_of!(Body, layer) == 40);
    assert!(core::mem::offset_of!(Body, pinned) == 44);
};

/// `{ x, y }`, the object literal `centroid` returns (`structs[1]` in the spec).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

const _: () = assert!(size_of::<Point>() == 16 && align_of::<Point>() == 8);

fn centroid(xs: &[f64], ys: &[f64]) -> Point {
    let mut sum = Vec2::new(0.0, 0.0);
    let n = math::min(array::len(xs), array::len(ys));
    let mut i = 0usize;
    while (i as f64) < n {
        // SAFETY: i < n = min(xs.len(), ys.len()).
        let point = unsafe { Vec2::new(array::get_unchecked(xs, i), array::get_unchecked(ys, i)) };
        sum = sum.add(point);
        i += 1;
    }
    let c = sum.scale(1.0 / n);
    Point { x: c.x, y: c.y }
}

fn spread(xs: &[f64], ys: &[f64]) -> f64 {
    let c = centroid(xs, ys);
    let mut widest = 0.0;
    let mut i = 0usize;
    while i < xs.len() {
        // SAFETY: i < xs.len(); `ys` may be shorter.
        let x = unsafe { array::get_unchecked(xs, i) };
        widest = math::max(widest, Vec2::new(x - c.x, array::get_usize(ys, i) - c.y).length());
        i += 1;
    }
    widest
}

fn step(body: &mut Body, ax: f64, ay: f64, dt: f64) {
    if body.pinned {
        return;
    }
    body.vx += ax * dt;
    body.vy += ay * dt;
    body.x += body.vx * dt;
    body.y += body.vy * dt;
}

fn collide(a: &mut Body, b: &mut Body) -> bool {
    if a.layer != b.layer {
        return false;
    }
    let d = Vec2::new(b.x - a.x, b.y - a.y);
    if d.length() > 1.0 {
        return false;
    }
    let total = a.mass + b.mass;
    let vx = (a.vx * a.mass + b.vx * b.mass) / total;
    let vy = (a.vy * a.mass + b.vy * b.mass) / total;
    a.vx = vx;
    a.vy = vy;
    b.vx = vx;
    b.vy = vy;
    a.mass += 0.5;
    b.mass += 0.25;
    true
}

fn momentum(a: &Body, b: &Body) -> f64 {
    Vec2::new(a.vx * a.mass + b.vx * b.mass, a.vy * a.mass + b.vy * b.mass).length()
}

#[unsafe(export_name = "centroid")]
pub unsafe extern "C" fn __abi_centroid(xs_ptr: u32, xs_len: u32, ys_ptr: u32, ys_len: u32) -> u32 {
    // SAFETY: the glue passes fresh copies.
    let point = unsafe { centroid(abi::slice(xs_ptr, xs_len), abi::slice(ys_ptr, ys_len)) };
    abi::return_struct(point)
}

#[unsafe(export_name = "spread")]
pub unsafe extern "C" fn __abi_spread(xs_ptr: u32, xs_len: u32, ys_ptr: u32, ys_len: u32) -> f64 {
    // SAFETY: the glue passes fresh copies.
    unsafe { spread(abi::slice(xs_ptr, xs_len), abi::slice(ys_ptr, ys_len)) }
}

#[unsafe(export_name = "step")]
pub unsafe extern "C" fn __abi_step(body_ptr: u32, ax: f64, ay: f64, dt: f64) {
    // SAFETY: the glue passes a fresh copy of a Body.
    step(unsafe { abi::struct_mut::<Body>(body_ptr) }, ax, ay, dt);
}

#[unsafe(export_name = "collide")]
pub unsafe extern "C" fn __abi_collide(a_ptr: u32, b_ptr: u32) -> i32 {
    // SAFETY: the glue passes fresh copies and calls the JS version when `a === b`.
    let (a, b) = unsafe { (abi::struct_mut::<Body>(a_ptr), abi::struct_mut::<Body>(b_ptr)) };
    i32::from(collide(a, b))
}

#[unsafe(export_name = "momentum")]
pub unsafe extern "C" fn __abi_momentum(a_ptr: u32, b_ptr: u32) -> f64 {
    // SAFETY: the glue passes fresh copies.
    unsafe { momentum(abi::struct_ref::<Body>(a_ptr), abi::struct_ref::<Body>(b_ptr)) }
}
