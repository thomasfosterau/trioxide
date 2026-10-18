import { gamma } from "./kernel.js";

const pixels = new Uint8ClampedArray(262144);

export const run = (): number => {
  for (let i = 0; i < 262144; i++) pixels[i] = i & 255;
  gamma(pixels);
  return pixels[1000] + pixels[77];
};

export function main(): void {
  console.log(run());
}
