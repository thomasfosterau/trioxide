import { crc32, fnv1a, packRgba, popcount, reverseBits, xorshift } from "./kernel.js";

const EDGES = [0, 1, -1, 0.9, -0.9, NaN, Infinity, 2 ** 31 - 1, 2 ** 31, -(2 ** 31), 2 ** 32 + 3, -(2 ** 53), 1e21, 0xdeadbeef];

export function main(): void {
  const text = new TextEncoder().encode("The quick brown fox jumps over the lazy dog");
  console.log(fnv1a(text), crc32(text), fnv1a(new Uint8Array(0)), crc32(new Uint8Array(0)));
  console.log(crc32(new Uint8Array(1000).map((_, i) => i * 7)), fnv1a(text.subarray(4, 9)));
  console.log(EDGES.map(popcount).join(" "));
  console.log(EDGES.map(reverseBits).join(" "));

  for (const seed of [42, 0, -1, 2 ** 32 + 7, NaN, 123.9]) {
    const out = new Int32Array(6);
    console.log(xorshift(seed, out), Array.from(out).join(" "));
  }
  const empty = new Int32Array(0);
  console.log(xorshift(5, empty), empty.length);

  console.log(packRgba(255, 128, 0, 255), packRgba(256, -1, 300.5, -0), packRgba(NaN, Infinity, 1e10, 2 ** 31));
}
