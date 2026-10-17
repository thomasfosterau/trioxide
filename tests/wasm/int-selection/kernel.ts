"use wasm";
// Locals and parameters become i32/u32 only where range facts prove every value fits;
// everything else is f64.

/** Sums 0..n-1 with an int32 counter; the sum may leave int32, so it stays f64. */
export function triangle(n: number): number {
  let sum = 0;
  for (let i = 0; i < n; i++) sum += i;
  return sum;
}

/** A counter bounded by a constant: i32. */
export function countBits(x: number): number {
  let count = 0;
  for (let bit = 0; bit < 32; bit++) {
    if ((x >>> bit) & 1) count++;
  }
  return count;
}

/** `.length`-bounded counter: typed array lengths can exceed int32, so only facts decide. */
export function sumBytes(bytes: Uint8Array): number {
  let total = 0;
  for (let i = 0; i < bytes.length; i++) total += bytes[i];
  return total;
}

/** Int arithmetic whose result may overflow int32 must not wrap. */
export function square(x: number): number {
  const y = x | 0;
  return y * y;
}

/** A uint32 result. */
export function hashPair(a: number, b: number): number {
  return ((a * 31) ^ b) >>> 0;
}

/** -0 can't be an int32. */
export function negate(x: number): number {
  const y = x | 0;
  return -y;
}
