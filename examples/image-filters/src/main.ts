import { boxBlur, grayscale, invert, sharpen } from "./filters.js";
import { checksum, testImage } from "./image.js";

const WIDTH = 320;
const HEIGHT = 240;

/** Runs every filter over the test image and returns a checksum per step. */
export function run(): number[] {
  const image = testImage(WIDTH, HEIGHT);
  const sums = [checksum(image)];
  const blurred = new Uint8ClampedArray(WIDTH * HEIGHT * 4);
  boxBlur(image, blurred, WIDTH, HEIGHT);
  sums.push(checksum(blurred));
  const sharp = new Uint8ClampedArray(WIDTH * HEIGHT * 4);
  sharpen(blurred, sharp, WIDTH, HEIGHT);
  sums.push(checksum(sharp));
  grayscale(sharp);
  sums.push(checksum(sharp));
  invert(sharp);
  sums.push(checksum(sharp));
  return sums;
}

export function main(): void {
  const names = ["source", "blur", "sharpen", "grayscale", "invert"];
  run().forEach((sum, i) => console.log(`${names[i]}: ${sum}`));
}
