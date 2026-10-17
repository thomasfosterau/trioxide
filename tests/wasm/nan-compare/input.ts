import { both, clamp, either, eq, ge, gt, le, looseEq, lt, ne, notLess, truthy } from "./kernel.js";

const show = (x: unknown): string => (Object.is(x, -0) ? "-0" : String(x));
const VALUES = [NaN, 0, -0, 1, -1, Infinity, -Infinity, 0.5];

export function main(): void {
  for (const a of VALUES) {
    const row: string[] = [];
    for (const b of VALUES) {
      row.push([lt(a, b), le(a, b), gt(a, b), ge(a, b), eq(a, b), ne(a, b), looseEq(a, b), notLess(a, b)].map((x) => (x ? 1 : 0)).join(""));
      row.push(show(either(a, b)), show(both(a, b)));
    }
    console.log(show(a), truthy(a), show(clamp(a, -1, 1)), show(clamp(a, NaN, 1)), row.join(" "));
  }
}
