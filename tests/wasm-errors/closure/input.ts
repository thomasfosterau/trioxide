// Closures aren't supported.
function apply(n: number): number {
  "use wasm";
  const double = (x: number) => x * 2;
  return double(n);
}

export function main(): void {
  console.log(apply(4));
}
