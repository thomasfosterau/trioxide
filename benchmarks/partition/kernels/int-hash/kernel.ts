// Integer mixing (xorshift and Math.imul) called once: all int32 compute.
export function hashRounds(seed: number, rounds: number): number {
  let h = seed | 0;
  let acc = 0;
  for (let i = 0; i < rounds; i++) {
    h ^= h << 13;
    h ^= h >> 17;
    h ^= h << 5;
    acc = (acc + Math.imul(h, 0x9e3779b1)) | 0;
  }
  return acc;
}
