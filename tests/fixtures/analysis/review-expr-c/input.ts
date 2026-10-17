// Object.prototype pollution through Object.getPrototypeOf.
function pollute(): void {
  const proto = Object.getPrototypeOf({});
  proto.extra = "polluted";
}

function readExtra(): void {
  const o: { a: number; extra?: number } = { a: 1 };
  const e = o.extra;
  console.log(e);
}

// Non-canonical index strings are not array indices.
function oddKeys(): void {
  const a = [1, 2, 3];
  const k = a["01"];
  const p = a["+1"];
  console.log(k, p);
}

// Tagged template strings.
function tag(strings: TemplateStringsArray, ...values: number[]): string {
  const first = strings[0];
  const n = strings.length;
  console.log(first, n, values.length);
  return "";
}

// Setter-only accessor read.
class SetOnly {
  set v(x: number) {
    console.log(x);
  }
}

// A constructor that returns another object.
const decoy = { v: 2 };
class Box {
  v = 1;
  constructor() {
    return decoy;
  }
}

// Unary plus over a declared parameter.
function plus(x: 1 | 2): number {
  const y = +x;
  return y;
}

export function main(): void {
  pollute();
  readExtra();
  oddKeys();
  tag`a${1}b`;
  const s = new SetOnly();
  const r = s.v;
  console.log(r);
  const b = new Box();
  b.v = 7;
  const dv = decoy.v;
  console.log(dv);
  (globalThis as any).plus = plus;
  (globalThis as any).plus("7");
}
