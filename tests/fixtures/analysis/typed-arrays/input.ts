// Typed arrays: element types follow the kind, whatever is stored.

export function fill(target: Uint8Array, value: number): void {
  for (let i = 0; i < target.length; i++) {
    target[i] = value;
  }
}

export function main(): void {
  const bytes = new Uint8Array(4);
  fill(bytes, 300);
  console.log(bytes[0] /*@proven number int nonNaN*/, bytes.length /*@proven number int*/);
  const floats = new Float32Array([1.5, 2.5]);
  console.log(floats[1] /*@proven number*/);
  const out = bytes[10];
  // Out of bounds: undefined.
  console.log(out /*@proven union*/);
  const view = bytes.subarray(1);
  console.log(view /*@proven typed-array*/);
  const big = new BigInt64Array(1);
  console.log(big[0] /*@proven bigint*/);
}
