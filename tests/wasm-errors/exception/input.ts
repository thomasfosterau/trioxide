// Exceptions can't cross the boundary.
function checked(n: number): number {
  "use wasm";
  if (n < 0) throw new RangeError("negative");
  return Math.sqrt(n);
}

export function main(): void {
  console.log(checked(4));
  try {
    checked(-1);
  } catch (error) {
    console.log((error as Error).name);
  }
}
