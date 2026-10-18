import { dot4 } from "./kernel.js";

const a = new Float32Array([1, 2, 3, 4]);
const b = new Float32Array([0.5, 0.25, 0.125, 2]);

export const run = (): number => {
  let sum = 0;
  for (let i = 0; i < 20_000; i++) {
    a[i & 3] = i & 7;
    sum += dot4(a, b);
  }
  return sum;
};

export function main(): void {
  console.log(run().toFixed(3));
}
