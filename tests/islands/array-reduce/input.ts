import { dot, normalize, prefixSums, stats, sum, weightedMean } from "./kernel.js";

const SAMPLES: number[][] = [
  [],
  [42],
  [3, -1, 4, 1, -5, 9, 2.5, -0],
  [-0, -0],
  [0.1, 0.2, 0.3, 1e16, -1e16],
  [NaN, 1, 2],
  [Infinity, -Infinity],
  [1.7976931348623157e308, 1.7976931348623157e308],
  Array.from({ length: 1000 }, (_, i) => Math.sin(i) * 1000),
];

export function main(): void {
  for (const values of SAMPLES) {
    console.log(values.length, sum(values), stats(values), prefixSums(values).slice(0, 6));
  }
  console.log(dot([1, 2, 3], [4, 5, 6]), dot([1, 2, 3], [4]), dot([], []), dot([-0], [1]));

  const prefix = prefixSums([1, 2, 3]);
  console.log(Array.isArray(prefix), prefix.length, prefix);
  const result = stats([5, 1]);
  console.log(Object.keys(result), JSON.stringify(result), result.constructor === Object);

  const values = [2, 6, -0, 2];
  normalize(values);
  console.log(values, Object.is(values[2], -0));
  const zeros = [0, -0];
  normalize(zeros);
  console.log(zeros);
  const same = [1, 2, 3];
  const before = same;
  normalize(same);
  console.log(before === same, same);

  console.log(weightedMean([1, 2, 3], new Float64Array([1, 1, 2])), weightedMean([1, 2], new Float64Array(1)));
}
