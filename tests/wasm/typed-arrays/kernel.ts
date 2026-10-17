"use wasm";
// Element writes convert like the typed array constructors do: ToInt8 … wrap, Uint8Clamped
// clamps and rounds half to even, Float32 rounds.

export function fillI8(xs: Int8Array, v: number): void { for (let i = 0; i < xs.length; i++) xs[i] = v * i; }
export function fillU8(xs: Uint8Array, v: number): void { for (let i = 0; i < xs.length; i++) xs[i] = v * i; }
export function fillC8(xs: Uint8ClampedArray, v: number): void { for (let i = 0; i < xs.length; i++) xs[i] = v * i; }
export function fillI16(xs: Int16Array, v: number): void { for (let i = 0; i < xs.length; i++) xs[i] = v * i; }
export function fillU16(xs: Uint16Array, v: number): void { for (let i = 0; i < xs.length; i++) xs[i] = v * i; }
export function fillI32(xs: Int32Array, v: number): void { for (let i = 0; i < xs.length; i++) xs[i] = v * i; }
export function fillU32(xs: Uint32Array, v: number): void { for (let i = 0; i < xs.length; i++) xs[i] = v * i; }
export function fillF32(xs: Float32Array, v: number): void { for (let i = 0; i < xs.length; i++) xs[i] = v * i; }
export function fillF64(xs: Float64Array, v: number): void { for (let i = 0; i < xs.length; i++) xs[i] = v * i; }

/** Reads widen back to numbers; sums cross element kinds. */
export function total(a: Int8Array, b: Uint16Array, c: Float32Array): number {
  let sum = 0;
  for (const x of a) sum += x;
  for (const x of b) sum += x;
  for (const x of c) sum += x;
  return sum;
}

/** A new array of each kind, returned. */
export function makeClamped(n: number): Uint8ClampedArray {
  const out = new Uint8ClampedArray(n);
  for (let i = 0; i < out.length; i++) out[i] = i * 100.5;
  return out;
}

export function makeFloat32(n: number): Float32Array {
  const out = new Float32Array(n);
  for (let i = 0; i < out.length; i++) out[i] = 1 / (i + 3);
  return out;
}
