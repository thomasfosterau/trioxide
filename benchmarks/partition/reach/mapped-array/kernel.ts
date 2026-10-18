// A number[] built by a builtin (`map`) rather than literals or `push`.
export function mean(values: number[]): number {
  let sum = 0;
  for (let i = 0; i < values.length; i++) sum += values[i];
  return sum / values.length;
}
