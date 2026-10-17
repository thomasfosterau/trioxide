// "use js" keeps a function out of every island, so its callers can't be compiled either.
function helper(x: number): number {
  "use js";
  return x * 3;
}

function caller(x: number): number {
  "use wasm";
  return helper(x) + 1;
}

export function main(): void {
  console.log(caller(2));
}
