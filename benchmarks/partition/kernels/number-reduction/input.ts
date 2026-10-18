import { rms } from "./kernel.js";

const values: number[] = [];
for (let i = 0; i < 50_000; i++) values.push(Math.sin(i) * 100);

export const run = (): number => {
  return rms(values);
};

export function main(): void {
  console.log(run().toFixed(6));
}
