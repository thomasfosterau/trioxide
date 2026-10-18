// A three-point stencil over a constant-size Float32Array (int32 counter).
export function conv(src: Float32Array, dst: Float32Array): void {
  for (let i = 1; i < 65535; i++) {
    dst[i] = 0.25 * src[i - 1] + 0.5 * src[i] + 0.25 * src[i + 1];
  }
}
