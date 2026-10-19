// `new C(…)` sites scalar must leave alone, and the close ones it may replace.
const log: string[] = [];
function note(text: string): number {
  log.push(text);
  return log.length;
}

class Pair {
  a: number;
  b: number;
  constructor(a: number, b: number, _unused?: number) {
    this.a = a;
    this.b = b;
  }
  sum(): number {
    return this.a + this.b;
  }
}

class Logged {
  v: number;
  constructor(v: number) {
    this.v = v;
    note("logged");
  }
}

class Base {
  v = 1;
}
class Derived extends Base {
  w = 2;
}

class WithGetter {
  v = 1;
  get w(): number {
    return this.v + 1;
  }
}

class Twice {
  a: number;
  b: number;
  constructor(x: number) {
    this.a = x;
    this.b = x;
  }
}

class Overridden {
  n = 1;
  u: number | undefined;
  constructor() {
    this.n = 2;
  }
}

let Swapped = class {
  v = 1;
};
class Reassigned {
  v = 1;
}
function swap(): void {
  // eslint-disable-next-line no-class-assign
  (Reassigned as any) = Swapped;
}

class Spot {
  v: number;
  constructor(v: number) {
    this.v = v;
  }
}

// Read before the class is declared: `new` throws, and must keep throwing.
function early(): number {
  try {
    const q = new Late(1);
    return q.v;
  } catch {
    return -1;
  }
}
const earlyResult = early();
class Late {
  v: number;
  constructor(v: number) {
    this.v = v;
  }
}

class Seen {
  v: number;
  constructor(v: number) {
    this.v = v;
  }
}

export function main(): void {
  const out: unknown[] = [earlyResult];
  // Ignored arguments with effects keep running.
  const p = new Pair(1, 2, note("extra"));
  out.push(p.a + p.b);
  // A pure ignored argument is dropped.
  const q = new Pair(3, 4, 5);
  out.push(q.a * q.b);
  // A method read (not called) isn't a field.
  const r = new Pair(5, 6);
  out.push(typeof r.sum);
  const l = new Logged(7);
  out.push(l.v);
  const d = new Derived();
  out.push(d.v + d.w);
  const g = new WithGetter();
  out.push(g.v);
  const t = new Twice(8);
  out.push(t.a + t.b);
  const o = new Overridden();
  out.push(o.n, o.u);
  if (log.length > 100) {
    swap();
  }
  const s = new Reassigned();
  out.push(s.v);
  // Parenthesised callee, arguments in order.
  const sp = new (Spot)(note("first") + note("second"));
  out.push(sp.v);
  // An anonymous function argument keeps its empty name.
  const f = new Spot((() => 1) as never);
  out.push((f.v as never as () => number).name.length);
  // Compared and passed: the object itself is used.
  const c = new Seen(10);
  out.push(c instanceof Seen, c.v);
  out.push(Swapped.name);
  console.log(JSON.stringify(out), log.join(","));
}
