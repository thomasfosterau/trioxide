"use wasm";
// A typed array whose `length` property isn't its real length (an own property, or a
// resizable buffer that grows), which JS loops follow and the island can't.

export function scale(xs: Float64Array, k: number): number {
  let n = 0;
  for (let i = 0; i < xs.length; i++) {
    xs[i] = xs[i] * k;
    n++;
  }
  return n;
}
