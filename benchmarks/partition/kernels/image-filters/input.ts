import { boxBlur, grayscale, sharpen } from "./kernel.js";

const WIDTH = 320;
const HEIGHT = 240;

function image(): Uint8ClampedArray {
  const pixels = new Uint8ClampedArray(WIDTH * HEIGHT * 4);
  for (let i = 0; i < pixels.length; i++) pixels[i] = (i * 37 + (i >> 2) * 11) % 256;
  return pixels;
}

const source = image();
const pixels = new Uint8ClampedArray(WIDTH * HEIGHT * 4);
const blurred = new Uint8ClampedArray(WIDTH * HEIGHT * 4);
const sharp = new Uint8ClampedArray(WIDTH * HEIGHT * 4);

export function run(): number {
  pixels.set(source);
  grayscale(pixels);
  boxBlur(pixels, blurred, WIDTH, HEIGHT);
  sharpen(blurred, sharp, WIDTH, HEIGHT);
  let sum = 0;
  for (let i = 0; i < sharp.length; i += 97) sum += sharp[i];
  return sum;
}

export function main(): void {
  console.log(run());
}
