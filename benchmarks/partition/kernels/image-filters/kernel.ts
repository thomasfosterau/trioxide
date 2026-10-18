// Typed-array image filters over RGBA pixels (ImageData's layout): the arrays are copied
// into the island and back.
export function grayscale(pixels: Uint8ClampedArray): void {
  for (let i = 0; i + 3 < pixels.length; i += 4) {
    const luma = 0.299 * pixels[i] + 0.587 * pixels[i + 1] + 0.114 * pixels[i + 2];
    pixels[i] = luma;
    pixels[i + 1] = luma;
    pixels[i + 2] = luma;
  }
}

export function boxBlur(src: Uint8ClampedArray, dst: Uint8ClampedArray, width: number, height: number): void {
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      for (let c = 0; c < 3; c++) {
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
      dst[(y * width + x) * 4 + 3] = src[(y * width + x) * 4 + 3];
    }
  }
}

export function sharpen(src: Uint8ClampedArray, dst: Uint8ClampedArray, width: number, height: number): void {
  for (let y = 1; y < height - 1; y++) {
    for (let x = 1; x < width - 1; x++) {
      const at = (y * width + x) * 4;
      for (let c = 0; c < 4; c++) {
        const i = at + c;
        dst[i] = 5 * src[i] - src[i - 4] - src[i + 4] - src[i - width * 4] - src[i + width * 4];
      }
    }
  }
}
