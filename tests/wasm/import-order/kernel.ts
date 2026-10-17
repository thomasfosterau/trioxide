"use wasm";
// Islands must not change when modules evaluate relative to other tasks (top-level await in
// the glue would), and calls made while modules evaluate, before the island is
// instantiated, run the JS versions.

export function scale(x: number, k: number): number {
  return x * k + 1;
}

console.log("kernel body", scale(2, 3));
