// A reduction over a number[]: the array is converted element by element at the boundary.
export function sumOfSquares(values: number[]): number {
  let sum = 0;
  for (let i = 0; i < values.length; i++) sum += values[i] * values[i];
  return sum;
}

export function rms(values: number[]): number {
  return Math.sqrt(sumOfSquares(values) / values.length);
}
