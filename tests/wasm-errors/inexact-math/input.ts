// Math.sin may differ from V8's in the last bit (docs/ABI.md, "Numerics"), which printing
// would show.
function wave(t: number): number {
  "use wasm";
  return Math.sin(t);
}

export function main(): void {
  console.log(wave(0.5));
}
