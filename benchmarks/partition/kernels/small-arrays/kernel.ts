// A little compute on a little typed array, called from a JS loop: copying and crossing
// cost about as much as the work.
export function dot4(a: Float32Array, b: Float32Array): number {
  let sum = 0;
  for (let i = 0; i < 4; i++) sum += a[i] * b[i];
  return sum;
}
