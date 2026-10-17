"use wasm";
// Reads the analysis can't prove in bounds use a checked helper: out of bounds they give
// NaN, which is what `undefined` gives wherever these values are used.

export function sumPairs(xs: Float64Array): number {
  let total = 0;
  for (let i = 0; i < xs.length; i += 2) total += xs[i] * xs[i + 1];
  return total;
}

export function at(xs: Int32Array, i: number): number {
  return xs[i] + 0;
}

/** `undefined < 1` and `NaN < 1` are both false; `xs[i] === 0` is false for both too. */
export function isSmall(xs: Float64Array, i: number): boolean {
  return xs[i] < 1;
}

export function isZero(xs: Float64Array, i: number): boolean {
  return xs[i] === 0;
}

/** Truthiness of a missing element is false either way. */
export function present(xs: Uint8Array, i: number): number {
  return xs[i] ? 1 : 0;
}

/** number[] reads past the end and of holes are undefined, NaN once used. */
export function dot(a: number[], b: number[]): number {
  let total = 0;
  for (let i = 0; i < a.length; i++) total += a[i] * b[i];
  return total;
}

/** Out-of-bounds writes to typed arrays are dropped. */
export function writeAll(xs: Int16Array, from: number, count: number): void {
  for (let i = 0; i < count; i++) xs[from + i] = 1000 * i;
}
