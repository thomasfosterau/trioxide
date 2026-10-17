"use wasm";
// NaN compares false except `!==`; negations stay as written.

export function lt(a: number, b: number): boolean { return a < b; }
export function le(a: number, b: number): boolean { return a <= b; }
export function gt(a: number, b: number): boolean { return a > b; }
export function ge(a: number, b: number): boolean { return a >= b; }
export function eq(a: number, b: number): boolean { return a === b; }
export function ne(a: number, b: number): boolean { return a !== b; }
export function looseEq(a: number, b: number): boolean { return a == b; }
export function notLess(a: number, b: number): boolean { return !(a < b); }

/** A NaN-safe clamp written with comparisons. */
export function clamp(x: number, lo: number, hi: number): number {
  return x < lo ? lo : x > hi ? hi : x;
}

/** Truthiness: 0, -0 and NaN are false. */
export function truthy(x: number): number {
  return x ? 1 : 0;
}

export function either(a: number, b: number): number {
  return a || b;
}

export function both(a: number, b: number): number {
  return a && b;
}
