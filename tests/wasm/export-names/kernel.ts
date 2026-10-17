"use wasm";
// Functions named like the runtime's own wasm exports, or with a `$`, which wasm symbols
// can't have: the glue keeps their names, the wasm module renames them.

export function memory(x: number): number {
  return x + 1;
}

export function __trioxide_alloc(x: number): number {
  return x * 2;
}

export function x$1(x: number): number {
  return x - 1;
}

export function x_1(x: number): number {
  return x * 10;
}
