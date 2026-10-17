// Nor be caught inside an island.
function safe(n: number): number {
  "use wasm";
  try {
    return n / 2;
  } catch {
    return 0;
  }
}

export function main(): void {
  console.log(safe(4));
}
