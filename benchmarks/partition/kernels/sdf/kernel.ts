// A signed-distance shape test per pixel: sqrt, abs, min and max, no arrays.
export function sdf(width: number, height: number): number {
  let inside = 0;
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const px = x / width - 0.5;
      const py = y / height - 0.5;
      const d1 = Math.sqrt(px * px + py * py) - 0.3;
      const d2 = Math.max(Math.abs(px) - 0.2, Math.abs(py) - 0.1);
      const d = Math.min(d1, d2);
      if (d < 0) inside++;
    }
  }
  return inside;
}
