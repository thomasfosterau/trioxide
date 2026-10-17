// Islands don't share state with JS: no writes to module-level variables.
let calls = 0;

function counted(x: number): number {
  "use wasm";
  calls++;
  return x;
}

export function main(): void {
  console.log(counted(1), calls);
}
