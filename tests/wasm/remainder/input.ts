import { irem, isEven, rem, wrapIndex } from "./kernel.js";

const show = (x: number | boolean): string => (Object.is(x, -0) ? "-0" : String(x));
const VALUES = [0, -0, 1, -1, 2, -2, 5.5, -5.5, 7, -7, 2 ** 31 - 1, -(2 ** 31), 1e300, NaN, Infinity, -Infinity, 5e-324];

export function main(): void {
  for (const a of VALUES) {
    console.log(show(a), VALUES.map((b) => `${show(rem(a, b))}/${show(irem(a, b))}`).join(" "), show(isEven(a)));
  }
  for (const i of [-7, -1, 0, 3, 10]) console.log(i, show(wrapIndex(i, 4)), show(wrapIndex(i, 0)), show(wrapIndex(i, -3)));
}
