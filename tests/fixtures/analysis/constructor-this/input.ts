// A constructor that only assigns `this.key` doesn't make its instance escape; one that
// passes `this` on does, however it does it, and then the shape and field types other code
// changes through that reference are not proven.
const registry: unknown[] = [];
function register(value: unknown): void {
  registry.push(value);
}
const holder: { ref: unknown } = { ref: null };
let last: unknown = null;

class Plain {
  x: number;
  constructor(x: number) {
    this.x = x;
  }
}

class Registered {
  x: unknown;
  constructor() {
    this.x = 1;
    register(this);
  }
}

class Stored {
  x: unknown;
  constructor() {
    this.x = 1;
    holder.ref = this;
  }
}

class Aliased {
  x: unknown;
  constructor() {
    this.x = 1;
    const self = this;
    last = self;
  }
}

class Captured {
  x: unknown;
  f: () => unknown;
  constructor() {
    this.x = 1;
    this.f = () => this;
  }
}

class Returned {
  x: unknown;
  constructor() {
    this.x = 1;
    if (registry.length > 100) {
      return;
    }
  }
}

function captured(): Captured {
  const c = new Captured();
  (globalThis as any).leakFn = c.f;
  return c;
}

export function main(): void {
  const p = new Plain(2);
  const r = new Registered();
  const s = new Stored();
  const a = new Aliased();
  const c = captured();
  const t = new Returned();
  (globalThis as any).sink = [registry, holder, last];
  const g = globalThis as any;
  g["si" + "nk"][0][0].x = "registered";
  g["si" + "nk"][0][0].added = true;
  g["si" + "nk"][1].ref.x = "stored";
  g["si" + "nk"][1].ref.added = true;
  g["si" + "nk"][2].x = "aliased";
  g["si" + "nk"][2].added = true;
  g["leak" + "Fn"]().x = "captured";
  g["leak" + "Fn"]().added = true;
  console.log(p.x, r.x, s.x, a.x, c.x, t.x);
  console.log(Object.keys(r), Object.keys(s), Object.keys(a), Object.keys(c));
}
