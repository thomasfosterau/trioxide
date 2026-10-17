import { axpy, determinant, identity, multiply, power, trace } from "./kernel.js";

const rows = (m: Float64Array, n: number): string =>
  Array.from({ length: n }, (_, i) => Array.from(m.subarray(i * n, i * n + n)).join(" ")).join(" | ");

export function main(): void {
  const a = new Float64Array([2, -1, 0, 1, 3, 2, 0.5, 4, -2]);
  const b = new Float64Array([1, 0, 1, -1, 2, 0.25, 3, 1, 1]);
  console.log(rows(identity(3), 3), identity(0).length);
  console.log(rows(multiply(a, b, 3), 3));
  console.log(rows(power(a, 3, 5), 3), rows(power(a, 3, 0), 3));
  const p = power(a, 3, 1);
  console.log(p === a, rows(p, 3));
  console.log(trace(a, 3), trace(a, 4), determinant(a, 3), determinant(b, 3));
  console.log(determinant(new Float64Array([1, 2, 2, 4]), 2), determinant(new Float64Array([0, 1, 1, 0]), 2), determinant(a, 0));
  console.log(rows(multiply(a, b, 2.5), 3), determinant(new Float64Array([NaN, 1, 1, 1]), 2));
  console.log(rows(power(new Float64Array([1, 1, 1, 0]), 2, 40), 2));

  const y = new Float64Array([1, 2, 3, 4, 5]);
  axpy(y, new Float64Array([1, 1, 1]), 2);
  console.log(Array.from(y));

  // Aliased arguments run the JS version, so updates see earlier writes as they do in JS.
  const v = new Float64Array([1, 2, 3, 4, 5]);
  axpy(v, v, 2);
  console.log(Array.from(v));
  axpy(v.subarray(1), v.subarray(0, 4), 1);
  console.log(Array.from(v));
  // Disjoint views of one buffer go to WASM.
  const w = new Float64Array([1, 2, 3, 4, 5, 6]);
  axpy(w.subarray(0, 3), w.subarray(3), -1);
  console.log(Array.from(w));
}
