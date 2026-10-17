import { bump, fillThenFail, sum, touch } from "./kernel.js";

const show = (xs: ArrayLike<unknown>): string =>
  Array.from({ length: xs.length }, (_, i) => (i in xs ? String(xs[i]) : "<hole>")).join(",");

export function main(): void {
  for (const n of [-5, 1.5, -Infinity]) {
    const dst = new Float64Array(3);
    try {
      console.log("ok", fillThenFail(dst, n));
    } catch (error) {
      console.log((error as Error).constructor.name, (error as Error).message);
    }
    console.log(show(dst));
  }

  const xs = new Float64Array([1, 2, 3]);
  structuredClone(xs.buffer, { transfer: [xs.buffer] });
  console.log(sum(xs), xs.length);
  bump(xs);
  console.log(xs.length);

  const holey: number[] = new Array(10);
  holey[3] = 5;
  holey[6] = 7;
  touch(holey);
  console.log(show(holey));
}
