// Islands: the JS version of a WASM island function is its fallback, so the speed passes leave
// it alone (its invariant `Math.sqrt(scale)` stays in the loop), while the same code outside
// the island is optimised.

function scaled(n: number, scale: number): number {
  "use wasm";
  let total = 0;
  for (let i = 0; i < n; i++) total += i * Math.sqrt(scale);
  return total;
}

function plain(n: number, scale: number): number {
  let total = 0;
  for (let i = 0; i < n; i++) total += i * Math.sqrt(scale);
  return total;
}

// Called at module evaluation too, before the island is ready.
const early = scaled(3, 4);

export function main(): void {
  console.log(early, scaled(4, 9), plain(4, 9), plain(2, 16));
}
