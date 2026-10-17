// An unchecked read is undefined out of bounds; the island would return NaN instead.
function element(xs: Int32Array, i: number): number {
  "use wasm";
  return xs[i];
}

export function main(): void {
  console.log(element(new Int32Array(2), 5));
}
