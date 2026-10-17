"use wasm";
// Every bitwise operator, on ints and on arbitrary numbers (ToInt32 / ToUint32 first).

export function and(a: number, b: number): number { return a & b; }
export function or(a: number, b: number): number { return a | b; }
export function xor(a: number, b: number): number { return a ^ b; }
export function not(a: number): number { return ~a; }
export function shl(a: number, b: number): number { return a << b; }
export function shr(a: number, b: number): number { return a >> b; }
export function ushr(a: number, b: number): number { return a >>> b; }
export function toInt(a: number): number { return a | 0; }
export function toUint(a: number): number { return a >>> 0; }
export function idiv(a: number, b: number): number { return ((a | 0) / (b | 0)) | 0; }
export function wrapSum(a: number, b: number): number { return ((a | 0) + (b | 0)) | 0; }
export function wrapDiff(a: number, b: number): number { return ((a | 0) - (b | 0)) | 0; }

/** Bit tricks on int32 locals. */
export function lowestBit(x: number): number {
  const v = x | 0;
  return v & -v;
}

export function rotate(x: number, n: number): number {
  const v = x | 0;
  const k = n & 31;
  return ((v << k) | (v >>> (32 - k))) | 0;
}
