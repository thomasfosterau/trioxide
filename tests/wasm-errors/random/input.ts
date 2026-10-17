// Math.random is host state.
function noise(scale: number): number {
  "use wasm";
  return Math.random() * scale;
}

export function main(): void {
  console.log(noise(0) === 0);
}
