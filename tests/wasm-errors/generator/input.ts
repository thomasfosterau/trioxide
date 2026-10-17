// Generators aren't supported.
function* count(n: number): Generator<number> {
  "use wasm";
  for (let i = 0; i < n; i++) yield i;
}

export function main(): void {
  console.log([...count(3)]);
}
