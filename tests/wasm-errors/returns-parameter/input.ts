// A returned array must be created by the call: the glue can't keep a parameter's identity.
function same(xs: Float64Array): Float64Array {
  "use wasm";
  xs[0] = 1;
  return xs;
}

export function main(): void {
  const xs = new Float64Array(2);
  console.log(same(xs) === xs);
}
