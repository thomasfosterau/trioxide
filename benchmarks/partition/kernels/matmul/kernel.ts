// Dense matrix multiply over Float64Arrays: O(n³) compute for O(n²) bytes copied.
export function matmul(a: Float64Array, b: Float64Array, out: Float64Array, n: number): void {
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < n; j++) {
      let sum = 0;
      for (let k = 0; k < n; k++) sum += a[i * n + k] * b[k * n + j];
      out[i * n + j] = sum;
    }
  }
}
