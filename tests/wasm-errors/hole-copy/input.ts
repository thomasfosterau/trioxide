// Storing an element of a number[] that may be a hole stores `undefined` in JS (the element
// stops being a hole), which an island can't represent; TypeScript's `number[]` doesn't
// rule holes out, so even with `trustTypes` the function stays JS.
function self(xs: number[]): void {
  "use wasm";
  for (let i = 0; i < xs.length; i++) xs[i] = xs[i];
}

function pick(xs: number[]): void {
  "use wasm";
  for (let i = 0; i < xs.length; i++) xs[i] = i > 0 ? xs[i] : 0;
}

export function main(): void {
  const a = [0, , 7] as number[];
  self(a);
  const b = [0, , 7] as number[];
  pick(b);
  console.log(a, b);
}
