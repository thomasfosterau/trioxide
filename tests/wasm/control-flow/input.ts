import { collatz, countdown, findPair, firstAbove, skipNegatives, sumOf } from "./kernel.js";

export function main(): void {
  const xs = new Float64Array([1, -2, 3.5, -0, 8, NaN, 2]);
  console.log(firstAbove(xs, 3), firstAbove(xs, 100), firstAbove(new Float64Array(0), 0), skipNegatives(xs));
  console.log(findPair(new Int32Array([1, 5, 3, 9, 4]), 7), findPair(new Int32Array([1, 2]), 10), findPair(new Int32Array([8, 1, 6]), 7));
  console.log([0, 1, 6, 27, 97, 2.5, -4, NaN].map(collatz).join(" "));
  console.log(countdown(10), countdown(0), countdown(2.5));
  console.log(sumOf([1, 2, 3]), sumOf([5, 200, 7]), sumOf([]), sumOf([1, , 2]));
}
