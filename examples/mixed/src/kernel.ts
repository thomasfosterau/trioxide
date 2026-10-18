// The compute kernel: escape-time statistics over a grid of the complex plane. Thousands of
// iterations per call and only numbers crossing the boundary, so the partitioner moves it
// to WebAssembly.

/** Total escape iterations over a `columns`×`rows` grid of `[x0, x1] × [y0, y1]`. */
export function escapeTotal(
  x0: number,
  y0: number,
  x1: number,
  y1: number,
  columns: number,
  rows: number,
  limit: number,
): number {
  let total = 0;
  for (let row = 0; row < rows; row++) {
    const cy = y0 + ((y1 - y0) * row) / rows;
    for (let column = 0; column < columns; column++) {
      total += escapeTime(x0 + ((x1 - x0) * column) / columns, cy, limit);
    }
  }
  return total;
}

/** How many iterations `z² + c` takes to leave the radius-2 disc, up to `limit`. */
export function escapeTime(cx: number, cy: number, limit: number): number {
  let x = 0;
  let y = 0;
  let i = 0;
  while (x * x + y * y <= 4 && i < limit) {
    const t = x * x - y * y + cx;
    y = 2 * x * y + cy;
    x = t;
    i++;
  }
  return i;
}
