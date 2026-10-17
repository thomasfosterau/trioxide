// Reductions over number[]: plain sums, an object-literal result, a new array, in-place
// updates, and reads past the end of a shorter array.

export function sum(values: number[]): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) total += values[i];
  return total;
}

export function stats(values: number[]): { min: number; max: number; mean: number } {
  let min = Infinity;
  let max = -Infinity;
  let total = 0;
  for (const value of values) {
    min = Math.min(min, value);
    max = Math.max(max, value);
    total += value;
  }
  return { min, max, mean: total / values.length };
}

/** Assumes `b` is at least as long as `a`; if not, the missing elements are NaN. */
export function dot(a: number[], b: number[]): number {
  let total = 0;
  for (let i = 0; i < a.length; i++) total += a[i] * b[i];
  return total;
}

export function normalize(values: number[]): void {
  const total = sum(values);
  for (let i = 0; i < values.length; i++) values[i] = values[i] / total;
}

export function prefixSums(values: number[]): number[] {
  const out = new Array<number>(values.length).fill(0);
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    total += values[i];
    out[i] = total;
  }
  return out;
}

export function weightedMean(values: number[], weights: Float64Array): number {
  let total = 0;
  let weight = 0;
  for (let i = 0; i < values.length; i++) {
    total += values[i] * weights[i];
    weight += weights[i];
  }
  return total / weight;
}
