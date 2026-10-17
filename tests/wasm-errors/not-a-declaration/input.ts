// Only top-level function declarations can be forced into WASM.
const twice = (x: number): number => {
  "use wasm";
  return x * 2;
};

export function main(): void {
  console.log(twice(2));
}
