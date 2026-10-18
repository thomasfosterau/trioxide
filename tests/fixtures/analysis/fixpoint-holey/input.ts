// A function called through an array, on a holey array a helper makes: storing into the
// array loses its length, and remaking it must not bring the length back (it used to, every
// round, so the analysis never reached a fixed point and reported nothing).
import { mul1 } from "./kernel.js";

function holey(): number[] {
  const xs: number[] = new Array(3);
  return xs;
}

export function main(): void {
  for (const f of [mul1]) {
    const xs = holey();
    f(xs);
    console.log(xs.length, xs[0], 0 in xs);
  }
}
