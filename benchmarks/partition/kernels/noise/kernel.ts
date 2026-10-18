// Hashed value noise per grid cell: xorshift rounds and `Math.imul` on int32s, but the
// cell coordinates are float counters (bounded by parameters), converted per call.
// The compute kernel: hashed value noise over a grid. Integer work (xorshift rounds and
// `Math.imul`) per cell and only numbers crossing the boundary, so the partitioner moves it
// to WebAssembly, where int32 arithmetic is about twice as fast as in V8's optimised JS.

/** How many cells of a `columns`×`rows` grid have noise above `level` (0..255). */
export function brightCells(seed: number, columns: number, rows: number, level: number): number {
  let count = 0;
  for (let y = 0; y < rows; y++) {
    for (let x = 0; x < columns; x++) {
      if (cellNoise(seed, x, y) > level) count++;
    }
  }
  return count;
}

/** The noise of cell (`x`, `y`): a hash of the seed and the coordinates, in 0..255. */
export function cellNoise(seed: number, x: number, y: number): number {
  let h = (seed ^ Math.imul(x, 0x27d4eb2d) ^ Math.imul(y, 0x165667b1)) | 0;
  for (let round = 0; round < 8; round++) {
    h ^= h << 13;
    h ^= h >> 17;
    h ^= h << 5;
    h = Math.imul(h, 0x9e3779b1);
  }
  return h >>> 24;
}
