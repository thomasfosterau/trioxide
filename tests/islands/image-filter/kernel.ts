// Filters over RGBA pixels in a Uint8ClampedArray, as in ImageData. Stores clamp to 0..255
// and round half to even, which the island must reproduce exactly.

export function brighten(pixels: Uint8ClampedArray, factor: number): void {
  for (let i = 0; i < pixels.length; i++) {
    if ((i & 3) !== 3) pixels[i] = pixels[i] * factor;
  }
}

export function grayscale(pixels: Uint8ClampedArray): void {
  for (let i = 0; i + 3 < pixels.length; i += 4) {
    const luma = 0.299 * pixels[i] + 0.587 * pixels[i + 1] + 0.114 * pixels[i + 2];
    pixels[i] = luma;
    pixels[i + 1] = luma;
    pixels[i + 2] = luma;
  }
}

export function boxBlur(src: Uint8ClampedArray, width: number, height: number): Uint8ClampedArray {
  const dst = new Uint8ClampedArray(src.length);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      for (let c = 0; c < 4; c++) {
        let sum = 0;
        let count = 0;
        for (let dy = -1; dy <= 1; dy++) {
          for (let dx = -1; dx <= 1; dx++) {
            const nx = x + dx;
            const ny = y + dy;
            if (nx >= 0 && nx < width && ny >= 0 && ny < height) {
              sum += src[(ny * width + nx) * 4 + c];
              count++;
            }
          }
        }
        dst[(y * width + x) * 4 + c] = sum / count;
      }
    }
  }
  return dst;
}

/** Counts colour values into 16 bins; alpha is skipped. */
export function histogram(pixels: Uint8ClampedArray, bins: Uint32Array): void {
  for (let i = 0; i < pixels.length; i++) {
    if ((i & 3) !== 3) bins[pixels[i] >> 4]++;
  }
}
