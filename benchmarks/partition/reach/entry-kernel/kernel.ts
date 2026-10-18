// A kernel the host calls: exported from the entry module, so nothing in the program shows
// what its arguments are.
export function scale(values: Float64Array, factor: number): void {
  for (let i = 0; i < values.length; i++) values[i] *= factor;
}
