// Per-pixel `Math.sqrt` over a constant-size Uint8ClampedArray (int32 counter).
export function gamma(pixels: Uint8ClampedArray): void {
  for (let i = 0; i < 262144; i++) {
    pixels[i] = Math.sqrt(pixels[i] / 255) * 255;
  }
}
