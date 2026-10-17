// for-of iterates the array it started with; reassigning the variable mid-loop isn't supported.
function sum(n: number): number {
  "use wasm";
  let xs = new Float64Array(3);
  let total = 0;
  for (const x of xs) {
    total += x * n;
    xs = new Float64Array(2);
  }
  return total;
}

export function main(): void {
  console.log(sum(2));
}
