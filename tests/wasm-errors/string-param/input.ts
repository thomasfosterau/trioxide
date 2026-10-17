// Parameters must be numbers, booleans, typed arrays, number[] or plain-data objects.
function size(text: string): number {
  "use wasm";
  return 1;
}

export function main(): void {
  console.log(size("abc"));
}
