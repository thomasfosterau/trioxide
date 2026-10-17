// Strings aren't supported in WASM (v1).
function label(n: number): number {
  "use wasm";
  const text = `#${n}`;
  return n + 1;
}

export function main(): void {
  console.log(label(1));
}
