import { smoothstep } from "./kernel.js";

const values = new Float64Array(100_000);
for (let i = 0; i < values.length; i++) values[i] = (i % 1000) / 1000;

export function run(): number {
  let sum = 0;
  for (let i = 0; i < values.length; i++) sum += smoothstep(0.2, 0.8, values[i]);
  return sum;
}

export function main(): void {
  console.log(run().toFixed(6));
}
