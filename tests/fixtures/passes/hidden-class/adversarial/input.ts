// Adversarial: where instances' keys can be observed, nothing changes.

class Keyed {
  a = 1;
  constructor(flag: boolean) {
    if (flag) this.b = 2;
  }
  b?: number;
  c?: number;
}

class Checked {
  value = 0;
}

class Shape {
  sides = 4;
  describe(): string {
    return `${this.sides} sides`;
  }
}

class Base {
  base = 1;
}

class Derived extends Base {
  extra?: number;
}

function keys(): string {
  const k = new Keyed(false) as Keyed & { c?: number };
  k.c = 3;
  // Key order and presence are observed.
  return `${Object.keys(k).join()} ${JSON.stringify(k)} ${"b" in k}`;
}

function checked(): string {
  const c = new Checked() as Checked & { later?: number };
  const before = "later" in c;
  c.later = 1;
  delete c.later;
  return `${before} ${"later" in c}`;
}

function inherited(): string {
  const s = new Shape();
  const before = s.describe();
  // An own `describe` added later: declaring it up front would hide the method.
  s.describe = () => "custom";
  return `${before} ${s.describe()}`;
}

function derived(): number {
  // Classes that extend, or are extended, are left alone.
  const d = new Derived();
  d.extra = 5;
  return d.base + (d.extra ?? 0) + new Base().base;
}

export function main(): void {
  console.log(keys(), checked(), inherited(), derived());
}
