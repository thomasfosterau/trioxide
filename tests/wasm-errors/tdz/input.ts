// `getN` runs before `N` is initialised, which throws in JS; an island would return 10.
function getN(i: number): number {
  "use wasm";
  return N * i;
}

let early = -1;
try {
  early = getN(2);
} catch {
  early = -2;
}

const N = 5;

export function main(): void {
  console.log(early, getN(3));
}
