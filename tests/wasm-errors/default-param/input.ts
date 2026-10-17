// Default values: the glue passes a missing argument as NaN, so `undefined` can't be seen.
function offset(x: number, by = 1): number {
  "use wasm";
  return x + by;
}

export function main(): void {
  console.log(offset(1), offset(1, 2));
}
