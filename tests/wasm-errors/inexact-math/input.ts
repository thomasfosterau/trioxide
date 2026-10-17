// Math.sin may differ from V8's in the last bit (docs/ABI.md, "Numerics"), which printing
// would show. So may sinh, atanh, log1p and atan2, on rarer inputs.
function wave(t: number): number {
  "use wasm";
  return Math.sin(t);
}

function rare(t: number): number {
  "use wasm";
  return Math.sinh(t) + Math.atanh(t) + Math.log1p(t) + Math.atan2(t, 1);
}

export function main(): void {
  console.log(wave(0.5), rare(0.5));
}
