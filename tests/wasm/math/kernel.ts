"use wasm";
// `Math` functions with JS-exact runtime helpers.

export function all(x: number, y: number): number[] {
  const out = new Array<number>(20).fill(0);
  out[0] = Math.abs(x);
  out[1] = Math.floor(x);
  out[2] = Math.ceil(x);
  out[3] = Math.trunc(x);
  out[4] = Math.sqrt(x);
  out[5] = Math.round(x);
  out[6] = Math.sign(x);
  out[7] = Math.fround(x);
  out[8] = Math.clz32(x);
  out[9] = Math.imul(x, y);
  out[10] = Math.max(x, y, 0);
  out[11] = Math.min(x, y);
  out[12] = Math.hypot(x, y);
  out[13] = Math.atan(y) + Math.asin(x / 1e300) + Math.acos(0.5);
  out[14] = Math.tan(x);
  out[15] = Math.log2(x);
  out[16] = Math.expm1(x);
  out[17] = x ** 2;
  out[18] = Math.pow(y, 1);
  out[19] = Math.PI * x + Math.max() + Math.min(y);
  return out;
}
