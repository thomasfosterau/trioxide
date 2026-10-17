import { boxBlur, brighten, grayscale, histogram } from "./kernel.js";

function image(width: number, height: number): Uint8ClampedArray {
  const pixels = new Uint8ClampedArray(width * height * 4);
  for (let i = 0; i < pixels.length; i++) pixels[i] = (i * 37 + (i >> 2) * 11) % 256;
  return pixels;
}

const dump = (pixels: ArrayLike<number>): string => Array.from(pixels).join(",");

export function main(): void {
  const img = image(5, 4);
  brighten(img, 1.3);
  console.log(dump(img));
  // Factors that land exactly on .5: ToUint8Clamp rounds half to even.
  brighten(img, 0.5);
  console.log(dump(img));
  brighten(img, 1.5);
  console.log(dump(img));
  grayscale(img);
  console.log(dump(img));

  const blurred = boxBlur(img, 5, 4);
  console.log(blurred.constructor.name, blurred.length, blurred.byteOffset, dump(blurred));
  // A width that doesn't fit the data: reads past the end are NaN and NaN stores as 0.
  console.log(dump(boxBlur(img, 2.5, 3)), dump(boxBlur(img, 7, 4)));
  console.log(boxBlur(new Uint8ClampedArray(0), 0, 0).length, boxBlur(img, NaN, 1).length);

  const bins = new Uint32Array(16);
  histogram(img, bins);
  console.log(dump(bins));
  // Too few bins: the increments past the end are dropped.
  const few = new Uint32Array(4);
  histogram(image(3, 3), few);
  console.log(dump(few));
  // Uint32 wraps.
  const full = new Uint32Array(16).fill(4294967295);
  histogram(new Uint8ClampedArray([0, 16, 255, 7]), full);
  console.log(dump(full));

  // Filtering a view filters the right part of the underlying buffer.
  const whole = image(4, 2);
  brighten(whole.subarray(4, 13), 2);
  console.log(dump(whole));

  // Bins overlapping the pixels they count: the island can't copy them separately, so the
  // glue calls the JS version instead.
  const shared = image(2, 2);
  histogram(shared, new Uint32Array(shared.buffer, 4, 2));
  console.log(dump(shared));
  // Views of one buffer that don't overlap go to WASM.
  const buffer = new ArrayBuffer(48);
  const pixels = new Uint8ClampedArray(buffer, 0, 16).fill(100);
  const disjoint = new Uint32Array(buffer, 16, 8);
  histogram(pixels, disjoint);
  console.log(dump(disjoint), dump(pixels));
}
