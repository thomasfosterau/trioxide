// Errors JS raises inside island code: typed array constructors reject invalid lengths with
// a RangeError, which the island reports as a trap and the glue rethrows as a RangeError.
// The island must keep working afterwards.

export function filled(length: number, value: number): Float64Array {
  "use wasm";
  const out = new Float64Array(length);
  for (let i = 0; i < out.length; i++) out[i] = value;
  return out;
}

export function total(length: number): number {
  "use wasm";
  const scratch = new Float64Array(length);
  let sum = 0;
  for (let i = 0; i < scratch.length; i++) {
    scratch[i] = i;
    sum += scratch[i];
  }
  return sum;
}
