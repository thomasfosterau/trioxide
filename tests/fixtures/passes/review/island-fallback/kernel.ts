// A forced island whose JS fallback the `ready()` glue calls by its exported name until the
// WASM is instantiated, and whose callers must keep calling it.
/** @trioxide wasm */
export function hot(x: number): number {
  return x * 2 + 1;
}

/** @trioxide wasm */
export function tagged(a: number, b: number): number {
  return a * b + 1;
}

export function run(n: number): number {
  let s = 0;
  for (let i = 0; i < n; i++) {
    s += hot(i);
  }
  return s;
}
