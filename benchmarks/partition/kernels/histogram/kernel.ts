// Counting bytes into a Uint32Array: little arithmetic per element access.
export function histogram(pixels: Uint8Array, bins: Uint32Array): void {
  for (let i = 0; i < 262144; i++) bins[pixels[i] >> 4]++;
}
