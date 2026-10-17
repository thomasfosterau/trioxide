// A `@trioxide wasm` JSDoc tag forces an island like the directive does.

/** @trioxide wasm */
function measure(xs: Float64Array): number {
  return xs.byteLength;
}

export function main(): void {
  console.log(measure(new Float64Array(3)));
}
