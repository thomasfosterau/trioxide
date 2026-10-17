// Host APIs (console, the DOM, timers, …) can't be called from an island.
function report(n: number): number {
  "use wasm";
  console.log(n);
  return n * 2;
}

export function main(): void {
  report(3);
}
