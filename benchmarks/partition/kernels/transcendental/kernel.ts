// Scalar math through `Math.atan` and `Math.log2` (libm in WASM), called once.
export function atans(n: number): number {
  let s = 0;
  for (let i = 0; i < n; i++) s += Math.atan(i * 0.001) + Math.log2(i + 1);
  return s;
}
