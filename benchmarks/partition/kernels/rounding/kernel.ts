// Math.round, Math.floor and Math.sign (JS-exact helpers in WASM) in a scalar loop.
export function rounds(n: number): number {
  let s = 0;
  for (let i = 0; i < n; i++) s += Math.round(i * 0.37) + Math.floor(i * 0.11) + Math.sign(i - 5000);
  return s;
}
