import { and, idiv, lowestBit, not, or, rotate, shl, shr, toInt, toUint, ushr, wrapDiff, wrapSum, xor } from "./kernel.js";

const VALUES = [0, -0, 1, -1, 7, -7, 0.5, -1.5, 31, 32, 33, -33, 2 ** 31 - 1, 2 ** 31, -(2 ** 31), 2 ** 32, 2 ** 32 + 5, 2 ** 53, 1e21, -1e21, NaN, Infinity, -Infinity, 0xdeadbeef];

export function main(): void {
  for (const a of VALUES) {
    const row: number[] = [not(a), toInt(a), toUint(a), lowestBit(a)];
    for (const b of [0, 1, 3, -1, 31, 32, 33, 0.5, NaN, 2 ** 32 + 1, 0xff00ff]) {
      row.push(and(a, b), or(a, b), xor(a, b), shl(a, b), shr(a, b), ushr(a, b), idiv(a, b), wrapSum(a, b), wrapDiff(a, b), rotate(a, b));
    }
    console.log(row.join(" "));
  }
}
