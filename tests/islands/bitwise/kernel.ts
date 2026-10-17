// Integer-proven loops: hashes, checksums and bit tricks that stay in int32/uint32.

export function fnv1a(bytes: Uint8Array): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < bytes.length; i++) {
    hash ^= bytes[i];
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

export function crc32(bytes: Uint8Array): number {
  const table = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c;
  }
  let crc = -1;
  for (let i = 0; i < bytes.length; i++) crc = table[(crc ^ bytes[i]) & 0xff] ^ (crc >>> 8);
  return (crc ^ -1) >>> 0;
}

export function popcount(x: number): number {
  let v = x | 0;
  let count = 0;
  while (v !== 0) {
    v &= v - 1;
    count++;
  }
  return count;
}

/** Fills `out` from a xorshift32 generator and returns the final state. */
export function xorshift(seed: number, out: Int32Array): number {
  let x = seed | 0 || 1;
  for (let i = 0; i < out.length; i++) {
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    out[i] = x;
  }
  return x;
}

export function reverseBits(x: number): number {
  let v = x >>> 0;
  let r = 0;
  for (let i = 0; i < 32; i++) {
    r = (r << 1) | (v & 1);
    v >>>= 1;
  }
  return r >>> 0;
}

export function packRgba(r: number, g: number, b: number, a: number): number {
  return (((a & 0xff) << 24) | ((b & 0xff) << 16) | ((g & 0xff) << 8) | (r & 0xff)) >>> 0;
}
