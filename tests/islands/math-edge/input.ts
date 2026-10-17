import {
  choose, clampByte, clz32, fround, hypot, idiv, imul, irem, isPositive, max, min3, mix, notLess,
  rem, round, sign, toInt32, toUint32, trunc, truthy, wrapAdd,
} from "./kernel.js";

const show = (x: unknown): string => (Object.is(x, -0) ? "-0" : String(x));

const VALUES = [
  NaN, 0, -0, Infinity, -Infinity, 5e-324, -5e-324, 2.2250738585072014e-308, 0.5, -0.5, 1.5, -1.5,
  2.5, -2.5, 0.49999999999999994, -0.7, 2 ** 31 - 1, 2 ** 31, -(2 ** 31), -(2 ** 31) - 1,
  2 ** 32, 2 ** 32 + 0.5, 4294967295.5, 2 ** 53, -1e21, 1e300, 123.456, -7, 3, 255.5, 254.5,
];
const PAIRS = [NaN, 0, -0, Infinity, -Infinity, 1, -1, 0.5, -2.5, 3, 31, 33, 2 ** 31, -(2 ** 32) - 3];
const INTS = [0, 1, -1, 7, -7, 3, -3, 2 ** 31 - 1, -(2 ** 31)];
const UINTS = [0, 1, 2 ** 31, 2 ** 32 - 1, 3e9];

export function main(): void {
  for (const x of VALUES) {
    const row = [round, sign, trunc, fround, clz32, toInt32, toUint32, isPositive, truthy, clampByte].map((f) => show(f(x)));
    console.log(show(x), row.join(" "));
  }
  for (const a of PAIRS) {
    const row: string[] = [];
    for (const b of PAIRS) {
      row.push([max(a, b), min3(a, b, -0), hypot(a, b, 2), rem(a, b), imul(a, b), mix(a, b), notLess(a, b)].map(show).join(","));
    }
    console.log(show(a), row.join(" "));
  }
  for (const a of INTS) {
    console.log(a, INTS.map((b) => `${show(idiv(a, b))}/${show(irem(a, b))}`).join(" "));
  }
  for (const a of UINTS) {
    console.log(a, UINTS.map((b) => wrapAdd(a, b)).join(" "));
  }
  console.log(choose(true, 1, 2), choose(false, 1, 2), choose(true, -0, 1), typeof isPositive(1), typeof clz32(1));
  console.log(round.name, round.length, choose.length, hypot.length);
}
