import { countBits, hashPair, negate, square, sumBytes, triangle } from "./kernel.js";

const show = (x: number): string => (Object.is(x, -0) ? "-0" : String(x));

export function main(): void {
  for (const n of [0, 1, 10, 100000, 2.5, -3, NaN]) console.log(n, triangle(n));
  for (const x of [0, 1, -1, 255, 2 ** 31 - 1, 2 ** 32 + 5, 0.5, NaN]) console.log(x, countBits(x));
  console.log(sumBytes(new Uint8Array([1, 2, 3, 250])), sumBytes(new Uint8Array(0)));
  for (const x of [3, -46341, 46341, 2 ** 31 - 1, -(2 ** 31), 1e10]) console.log(x, square(x));
  for (const [a, b] of [[1, 2], [2 ** 31, 7], [-1, -1], [123456789, 987654321]]) console.log(a, b, hashPair(a, b));
  for (const x of [0, 1, -1, -(2 ** 31), 0.5]) console.log(x, show(negate(x)));
}
