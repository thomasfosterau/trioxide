"use wasm";
// `%` is fmod on numbers; on int32 operands the result is still a number (-0, NaN).

export function rem(a: number, b: number): number {
  return a % b;
}

export function irem(a: number, b: number): number {
  return (a | 0) % (b | 0);
}

/** Remainders feeding int32 contexts and comparisons. */
export function isEven(x: number): boolean {
  return (x | 0) % 2 === 0;
}

export function wrapIndex(i: number, n: number): number {
  return ((i % n) + n) % n;
}
