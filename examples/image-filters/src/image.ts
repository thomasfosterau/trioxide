// A procedural RGBA test image, so the example needs no files.

/** A `width`×`height` RGBA image of gradients and rings. */
export function testImage(width: number, height: number): Uint8ClampedArray {
  const pixels = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const at = (y * width + x) * 4;
      const dx = x - width / 2;
      const dy = y - height / 2;
      const ring = Math.sqrt(dx * dx + dy * dy) % 32;
      pixels[at] = (x * 255) / width;
      pixels[at + 1] = (y * 255) / height;
      pixels[at + 2] = ring * 8;
      pixels[at + 3] = 255;
    }
  }
  return pixels;
}

/** A checksum of `pixels`, to compare runs. */
export function checksum(pixels: Uint8ClampedArray): number {
  let sum = 0;
  for (let i = 0; i < pixels.length; i++) sum = (sum * 31 + pixels[i]) % 1_000_000_007;
  return sum;
}
