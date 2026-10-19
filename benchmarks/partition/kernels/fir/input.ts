import { fir } from "./kernel.js";

const FRAME = 2048;
const taps = new Float64Array(63);
for (let i = 0; i < taps.length; i++) taps[i] = 1 / (1 + Math.abs(i - 31));

export const run = (): number => {
  const input = new Float64Array(FRAME);
  const output = new Float64Array(FRAME);
  let sum = 0;
  for (let frame = 0; frame < 8; frame++) {
    for (let i = 0; i < FRAME; i++) input[i] = Math.sin((frame * FRAME + i) * 0.01);
    fir(input, taps, output);
    for (let i = 0; i < FRAME; i++) sum += output[i];
  }
  return Math.round(sum * 1e6);
};

export function main(): void {
  console.log(run());
}
