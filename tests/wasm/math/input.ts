import { all } from "./kernel.js";

const show = (x: number): string => (Object.is(x, -0) ? "-0" : String(x));
const VALUES = [NaN, 0, -0, 1, -1, 0.5, -0.5, 2.5, -2.5, 1e300, -1e-300, Infinity, -Infinity, 123.456, 2 ** 31, -(2 ** 32) + 1];

export function main(): void {
  for (const x of VALUES) {
    for (const y of [0, -0, 2, NaN, -7.25]) console.log(show(x), show(y), all(x, y).map(show).join(" "));
  }
}
