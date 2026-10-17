// JS-exact numerics across the boundary: every scalar ABI type in and out, on edge values.

export function round(x: number): number {
  return Math.round(x);
}

export function sign(x: number): number {
  return Math.sign(x);
}

export function trunc(x: number): number {
  return Math.trunc(x);
}

export function fround(x: number): number {
  return Math.fround(x);
}

export function max(a: number, b: number): number {
  return Math.max(a, b);
}

export function min3(a: number, b: number, c: number): number {
  return Math.min(a, b, c);
}

export function hypot(a: number, b: number, c: number): number {
  return Math.hypot(a, b, c);
}

export function rem(a: number, b: number): number {
  return a % b;
}

export function clz32(x: number): number {
  return Math.clz32(x);
}

export function imul(a: number, b: number): number {
  return Math.imul(a, b);
}

export function toInt32(x: number): number {
  return x | 0;
}

export function toUint32(x: number): number {
  return x >>> 0;
}

export function mix(a: number, b: number): number {
  return (a << 3) ^ (b >> 1) ^ (~a >>> 28);
}

/** Integer division; callers pass int32s. */
export function idiv(a: number, b: number): number {
  return (a / b) | 0;
}

/** Integer remainder; callers pass int32s. */
export function irem(a: number, b: number): number {
  return a % b;
}

/** Wrapping uint32 addition; callers pass uint32s. */
export function wrapAdd(a: number, b: number): number {
  return (a + b) >>> 0;
}

export function isPositive(x: number): boolean {
  return x > 0;
}

export function notLess(a: number, b: number): boolean {
  return !(a < b);
}

export function choose(flag: boolean, a: number, b: number): number {
  return flag ? a : b;
}

export function truthy(x: number): boolean {
  return !!x;
}

export function clampByte(x: number): number {
  const out = new Uint8ClampedArray(1);
  out[0] = x;
  return out[0];
}
