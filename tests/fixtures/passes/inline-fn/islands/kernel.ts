// WASM island functions: never inlined, evaluated at compile time, or removed, and nothing is
// inlined into them.
function square(x: number): number {
  return x * x;
}

/** @trioxide wasm */
export function hot(x: number): number {
  return square(x) + 1;
}

/** @trioxide wasm */
export function table(n: number): number {
  let s = 0;
  for (let i = 0; i < n; i++) {
    s += i * 2;
  }
  return s;
}

export function run(n: number): number {
  let s = 0;
  for (let i = 0; i < n; i++) {
    const p = { x: i, y: 1 };
    s += hot(p.x) * p.y;
  }
  return s;
}
