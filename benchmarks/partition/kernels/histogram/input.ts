import { histogram } from "./kernel.js";

const pixels = new Uint8Array(262144);
for (let i = 0; i < pixels.length; i++) pixels[i] = (i * 31) & 255;
const bins = new Uint32Array(16);

export const run = (): number => {
  bins.fill(0);
  histogram(pixels, bins);
  return bins[3];
};

export function main(): void {
  console.log(run());
}
