// Writing `__proto__` changes an object's prototype: an instance stops being one of its
// class, and its "fields" don't include `__proto__` (no own property is made).
const proto = { inherited: 9, kind: "proto" };

class InConstructor {
  constructor(p: object) {
    (this as any).__proto__ = p;
  }
}

class Later {
  v = 1;
}

class ViaSet {
  v = 1;
}

class ViaComputed {
  v = 1;
}

class Kept {
  v = 1;
}

function computedKey(): string {
  return "__pro" + "to__";
}

export function main(): void {
  const a = new InConstructor(proto);
  const b = new Later();
  (b as any).__proto__ = proto;
  const c = new ViaSet();
  Object.setPrototypeOf(c, proto);
  const d = new ViaComputed();
  (d as any)["__proto__"] = proto;
  const e = new ViaComputed();
  (e as any)[computedKey()] = proto;
  // Never given away nor reprototyped: still a `Kept`.
  const kept = new Kept();
  const keptV = kept.v;
  const literal = { v: 1 };
  (literal as any).__proto__ = proto;
  console.log(a instanceof InConstructor, (a as any).inherited, Object.keys(a));
  console.log(b instanceof Later, b.v, (b as any).kind, Object.keys(b));
  console.log(c instanceof ViaSet, c.v, (c as any).kind);
  console.log(d instanceof ViaComputed, d.v, (d as any).kind);
  console.log(e instanceof ViaComputed, e.v, (e as any).kind);
  console.log((literal as any).kind, Object.keys(literal), keptV);
}
