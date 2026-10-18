import { clamp, lerp } from "./kernel.js";

export function run(): number {
  let sum = 0;
  for (let i = 0; i < 1000; i++) sum += clamp(lerp(-1, 2, i / 1000), 0, 1);
  return sum + clamp(5, 0, 3);
}

export function main(): void {
  console.log(run().toFixed(6));
}
