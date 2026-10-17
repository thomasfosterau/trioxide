"use wasm";
// Loops of every shape, labelled jumps, `continue` with an update, and early returns.

export function firstAbove(xs: Float64Array, limit: number): number {
  for (let i = 0; i < xs.length; i++) {
    if (xs[i] > limit) return i;
  }
  return -1;
}

export function skipNegatives(xs: Float64Array): number {
  let total = 0;
  for (let i = 0; i < xs.length; i++) {
    if (xs[i] < 0) continue;
    total += xs[i];
  }
  return total;
}

export function findPair(xs: Int32Array, target: number): number {
  let found = -1;
  outer: for (let i = 0; i < xs.length; i++) {
    for (let j = i + 1; j < xs.length; j++) {
      if (xs[i] + xs[j] === target) {
        found = i * 1000 + j;
        break outer;
      }
      if (xs[j] > target) continue outer;
    }
  }
  return found;
}

export function collatz(n: number): number {
  let steps = 0;
  let x = n;
  do {
    if (x <= 1) break;
    x = x % 2 === 0 ? x / 2 : 3 * x + 1;
    steps++;
  } while (steps < 1000);
  return steps;
}

export function countdown(n: number): number {
  let total = 0;
  let i = n;
  while (true) {
    i--;
    if (i < 0) break;
    if (i % 3 === 0) continue;
    total += i;
  }
  return total;
}

export function sumOf(values: number[]): number {
  let total = 0;
  for (const v of values) {
    if (v > 100) break;
    total += v;
  }
  return total;
}

// `continue` in a loop with an update becomes `break` out of a labelled block, so a `break`
// of the same loop inside it needs a Rust label; so does one leaving a labelled JS block.
export function jumps(limit: number): number {
  "use wasm";
  let total = 0;
  let m = 0;
  do {
    m++;
    if (m === 2) continue;
    if (total > limit) break;
    total += m;
  } while (m < 10);
  for (let i = 0; i < 8; i++) {
    if (i % 3 === 0) continue;
    found: {
      if (i > limit) break found;
      if (i === 7) break;
      total += i * 10;
    }
  }
  return total;
}

// An unchecked read starting an operand (`unsafe { … } + 1`) must not parse as a statement.
export function pick(xs: Float64Array, k: number): number {
  "use wasm";
  let total = 0;
  for (let i = 0; i < xs.length; i++) total += k > i ? 1 : xs[i] + 0.5;
  return total;
}
