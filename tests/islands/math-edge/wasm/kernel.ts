// JS-exact numerics across the boundary: every scalar ABI type in and out, on edge values.

export function round(x: number): number {
  "use wasm";
  return Math.round(x);
}

export function sign(x: number): number {
  "use wasm";
  return Math.sign(x);
}

export function trunc(x: number): number {
  "use wasm";
  return Math.trunc(x);
}

export function fround(x: number): number {
  "use wasm";
  return Math.fround(x);
}

export function max(a: number, b: number): number {
  "use wasm";
  return Math.max(a, b);
}

export function min3(a: number, b: number, c: number): number {
  "use wasm";
  return Math.min(a, b, c);
}

export function hypot(a: number, b: number, c: number): number {
  "use wasm";
  return Math.hypot(a, b, c);
}

export function rem(a: number, b: number): number {
  "use wasm";
  return a % b;
}

export function clz32(x: number): number {
  "use wasm";
  return Math.clz32(x);
}

export function imul(a: number, b: number): number {
  "use wasm";
  return Math.imul(a, b);
}

export function toInt32(x: number): number {
  "use wasm";
  return x | 0;
}

export function toUint32(x: number): number {
  "use wasm";
  return x >>> 0;
}

export function mix(a: number, b: number): number {
  "use wasm";
  return (a << 3) ^ (b >> 1) ^ (~a >>> 28);
}

/** Integer division; callers pass int32s. */
export function idiv(a: number, b: number): number {
  "use wasm";
  return (a / b) | 0;
}

/** Integer remainder; callers pass int32s. */
export function irem(a: number, b: number): number {
  "use wasm";
  return a % b;
}

/** Wrapping uint32 addition; callers pass uint32s. */
export function wrapAdd(a: number, b: number): number {
  "use wasm";
  return (a + b) >>> 0;
}

export function isPositive(x: number): boolean {
  "use wasm";
  return x > 0;
}

export function notLess(a: number, b: number): boolean {
  "use wasm";
  return !(a < b);
}

export function choose(flag: boolean, a: number, b: number): number {
  "use wasm";
  return flag ? a : b;
}

export function truthy(x: number): boolean {
  "use wasm";
  return !!x;
}

export function clampByte(x: number): number {
  "use wasm";
  const out = new Uint8ClampedArray(1);
  out[0] = x;
  return out[0];
}
