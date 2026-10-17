"use wasm";
// What the glue must get right besides plain calls: writes made before an exception stay
// visible, detached buffers act as empty arrays, and holes in a number[] stay holes unless
// the island writes them.

export function fillThenFail(dst: Float64Array, n: number): number {
  dst[0] = 1;
  const scratch = new Float64Array(n);
  dst[1] = 2;
  return scratch.length;
}

export function sum(xs: Float64Array): number {
  let total = 0;
  for (let i = 0; i < xs.length; i++) total += xs[i];
  return total;
}

export function bump(xs: Float64Array): void {
  for (let i = 0; i < xs.length; i++) xs[i] += 1;
}

export function touch(xs: number[]): void {
  for (let i = 0; i < xs.length; i++) {
    if (i % 4 === 1) xs[i] = NaN;
    else if (i % 4 === 2) xs[i] += 0;
    else if (i % 4 === 3) xs[i] = +xs[i];
  }
}
