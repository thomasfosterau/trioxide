// `switch` isn't supported yet.
function pick(n: number): number {
  "use wasm";
  switch (n) {
    case 1:
      return 10;
    default:
      return 0;
  }
}

export function main(): void {
  console.log(pick(1));
}
