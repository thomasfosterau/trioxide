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
