"use wasm";
// Integer work on typed array elements stays in integer arithmetic: `xs[i]++` and `xs[i] += k`
// in the element type's own wrapping arithmetic, `xs[i] | 0` without a round trip through a
// number, and `ToInt32` of constants folded (`Math.imul(h, 0x9e3779b1)`).

export function bumpI8(xs: Int8Array, i: number): void { xs[i]++; }
export function bumpU8(xs: Uint8Array, i: number): void { xs[i]++; }
export function bumpC8(xs: Uint8ClampedArray, i: number): void { xs[i]++; }
export function bumpI16(xs: Int16Array, i: number): void { xs[i]--; }
export function bumpU16(xs: Uint16Array, i: number): void { xs[i]--; }
export function bumpI32(xs: Int32Array, i: number): void { xs[i] += 7; }
export function bumpU32(xs: Uint32Array, i: number): void { xs[i] -= 3; }
export function bumpF32(xs: Float32Array, i: number): void { xs[i]++; }
export function bumpF64(xs: Float64Array, i: number): void { xs[i] -= 1; }

/** `+=` of an int32 that isn't a constant, and `-=` of one (written out in `f64`). */
export function addAll(xs: Uint32Array, k: number): void {
  const d = k | 0;
  for (let i = 0; i < xs.length; i++) {
    xs[i] += d;
    xs[i] -= d >> 1;
  }
}

/** Counting into bins: `bins[b]++` with an index from another array. */
export function histogram(bytes: Uint8Array, bins: Uint32Array): void {
  for (let i = 0; i < bytes.length; i++) bins[bytes[i] >> 4]++;
}

/** The right-hand side writes the element first: JS adds to the value read before it. */
function overwrite(xs: Int32Array, i: number): number {
  xs[i] = 100;
  return 1;
}

export function addAfterWrite(xs: Int32Array, i: number): void {
  xs[i] += overwrite(xs, i);
}

/** `ToInt32` of reads, of every kind, in and out of bounds. */
export function readInts(xs: Float64Array, ys: Uint32Array, zs: Int8Array, i: number): number {
  return ((xs[i] | 0) ^ (ys[i] | 0)) + (zs[i] >> 1) + (xs[i + 100] | 0) + (ys[-1] & 0xff);
}

/** Constants above 2^31, negative, fractional and non-finite under `ToInt32`. */
export function hash(seed: number, rounds: number): number {
  let h = seed | 0;
  for (let r = 0; r < rounds; r++) {
    h ^= h << 13;
    h = Math.imul(h, 0x9e3779b1);
    h = Math.imul(h ^ 4294967301.5, -2.5) + Math.imul(1e10, h);
    h = (h & 0xffffffff) ^ (h | Infinity) ^ (h >>> 0x100000020);
  }
  return h >>> 0;
}
