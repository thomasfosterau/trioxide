import { matmul } from "./kernel.js";

const N = 96;
const a = new Float64Array(N * N);
const b = new Float64Array(N * N);
const out = new Float64Array(N * N);
for (let i = 0; i < N * N; i++) {
  a[i] = (i % 17) / 7;
  b[i] = (i % 13) / 5;
}

export const run = (): number => {
  matmul(a, b, out, N);
  return out[0] + out[N * N - 1] + out[(N * N) >> 1];
};

export function main(): void {
  console.log(run().toFixed(6));
}
