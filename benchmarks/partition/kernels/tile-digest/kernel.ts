// A digest of hashed value noise over a fixed-size tile: int32 work only (`Math.imul`,
// xorshift, `(a + b) | 0` wrapping) on int32 counters, one call per tile.

/** The tile's width and height in cells. */
const TILE = 256;

/** A 32-bit digest of the noise of every cell of a tile, for a seed. */
export function tileDigest(seed: number): number {
  const key = seed | 0;
  let digest = 0;
  for (let y = 0; y < TILE; y++) {
    for (let x = 0; x < TILE; x++) {
      digest = (Math.imul(digest, 31) + cellNoise(key, x, y)) | 0;
    }
  }
  return digest >>> 0;
}

/** The noise of cell (`x`, `y`): a hash of the seed and the coordinates, in 0..255. */
export function cellNoise(seed: number, x: number, y: number): number {
  let h = Math.imul(x, 374761393) ^ Math.imul(y, 668265261) ^ seed;
  for (let round = 0; round < 4; round++) {
    h ^= h << 13;
    h ^= h >> 17;
    h ^= h << 5;
    h = (h + Math.imul(h, 1597334677)) | 0;
  }
  return h >>> 24;
}
