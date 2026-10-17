import { at, dot, isSmall, isZero, present, sumPairs, writeAll } from "./kernel.js";

export function main(): void {
  console.log(sumPairs(new Float64Array([1, 2, 3, 4])), sumPairs(new Float64Array([1, 2, 3])), sumPairs(new Float64Array(0)));
  const ints = new Int32Array([5, -6, 7]);
  console.log([-1, 0, 2, 3, 1.5, NaN, 2 ** 32].map((i) => at(ints, i)).join(" "));
  const xs = new Float64Array([0, 0.5, 2, NaN]);
  console.log([-1, 0, 1, 2, 3, 4].map((i) => `${isSmall(xs, i)}/${isZero(xs, i)}`).join(" "));
  console.log([0, 1, 2, 5, -1].map((i) => present(new Uint8Array([0, 9]), i)).join(" "));
  console.log(dot([1, 2, 3], [4, 5, 6]), dot([1, 2, 3], [4]), dot([], [1]), dot([1, , 3], [1, 1, 1]));
  const shorts = new Int16Array(4);
  writeAll(shorts, 2, 5);
  writeAll(shorts, -1, 2);
  console.log(Array.from(shorts).join(" "));
}
