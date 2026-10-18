// A tight numeric kernel, called once: all compute, no arrays crossing the boundary.
export function mandelbrot(width: number, height: number, iterations: number): number {
  let inside = 0;
  for (let py = 0; py < height; py++) {
    for (let px = 0; px < width; px++) {
      const cx = (px / width) * 3.5 - 2.5;
      const cy = (py / height) * 2 - 1;
      let x = 0;
      let y = 0;
      let i = 0;
      while (x * x + y * y <= 4 && i < iterations) {
        const t = x * x - y * y + cx;
        y = 2 * x * y + cy;
        x = t;
        i++;
      }
      if (i === iterations) inside++;
    }
  }
  return inside;
}
