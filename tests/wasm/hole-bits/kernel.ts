"use wasm";
// Holes in a number[] enter the island as a signalling NaN that marks them unwritten. Values
// stored into a number[] must never carry those bits, whatever produced them: `* 1`, `- 0`,
// `/ 1` and `-(-x)` fold to `x`, `-x` and `Math.abs` only touch the sign, and a typed array
// can hold the pattern itself.

export function mul1(xs: number[]): void {
  for (let i = 0; i < xs.length; i++) xs[i] = xs[i] * 1;
}

export function sub0(xs: number[]): void {
  for (let i = 0; i < xs.length; i++) xs[i] = xs[i] - 0;
}

export function div1(xs: number[]): void {
  for (let i = 0; i < xs.length; i++) xs[i] = xs[i] / 1;
}

export function subAssign0(xs: number[]): void {
  for (let i = 0; i < xs.length; i++) xs[i] -= 0;
}

export function negNeg(xs: number[]): void {
  for (let i = 0; i < xs.length; i++) xs[i] = -(-xs[i]);
}

export function absSelf(xs: number[]): void {
  for (let i = 0; i < xs.length; i++) xs[i] = Math.abs(xs[i]);
}

export function absInto(ys: number[], fa: Float64Array): void {
  for (let i = 0; i < ys.length; i++) ys[i] = Math.abs(fa[i]);
}

export function negInto(ys: number[], fa: Float64Array): void {
  for (let i = 0; i < ys.length; i++) ys[i] = -fa[i];
}
