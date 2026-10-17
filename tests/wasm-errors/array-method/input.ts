// Array methods aren't supported.
function grow(values: Float64Array): number {
  "use wasm";
  return values.indexOf(2);
}

export function main(): void {
  console.log(grow(new Float64Array([1, 2])));
}
