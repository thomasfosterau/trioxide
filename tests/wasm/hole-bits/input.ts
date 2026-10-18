import { absInto, absSelf, div1, mul1, negInto, negNeg, sub0, subAssign0 } from "./kernel.js";

const show = (xs: ArrayLike<unknown>): string =>
  Array.from({ length: xs.length }, (_, i) => (i in xs ? String(xs[i]) : "<hole>")).join(",");

function holey(): number[] {
  const xs: number[] = new Array(3);
  xs[0] = 7;
  xs[2] = -7;
  return xs;
}

export function main(): void {
  // Direct calls: a loop over an array of functions doesn't converge in the analysis yet.
  const results = [mul1, sub0, div1, subAssign0, negNeg, absSelf].map((f) => f.name);
  let xs = holey();
  mul1(xs);
  console.log(results[0], show(xs), 1 in xs);
  xs = holey();
  sub0(xs);
  console.log(results[1], show(xs), 1 in xs);
  xs = holey();
  div1(xs);
  console.log(results[2], show(xs), 1 in xs);
  xs = holey();
  subAssign0(xs);
  console.log(results[3], show(xs), 1 in xs);
  xs = holey();
  negNeg(xs);
  console.log(results[4], show(xs), 1 in xs);
  xs = holey();
  absSelf(xs);
  console.log(results[5], show(xs), 1 in xs);
  const fa = new Float64Array(3);
  const bits = new BigUint64Array(fa.buffer);
  bits[0] = 0xfff4_7472_696f_7869n;
  bits[1] = 0x7ff4_7472_696f_7869n;
  fa[2] = 3;
  let ys: number[] = [5, 5, 5];
  absInto(ys, fa);
  console.log("absInto", show(ys));
  ys = [5, 5, 5];
  negInto(ys, fa);
  console.log("negInto", show(ys));
}
