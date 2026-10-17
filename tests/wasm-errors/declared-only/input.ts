// An entry module's exports are called by the host: their parameters are only what
// TypeScript says (Declared), which isn't proof unless `trustTypes` is on.
export function scale(x: number, k: number): number {
  "use wasm";
  return x * k;
}

export function main(): void {
  console.log(scale(2, 3));
}
