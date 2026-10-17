// A subclass accessor shadowed by the parent's own field (define semantics).
class Base {
  x: number | string = 1;
}
class Derived extends Base {
  // @ts-ignore
  get x(): string {
    return "getter";
  }
}

// A typed array detached through its buffer.
function detach(): void {
  const ta = new Float64Array(4);
  const buffer: any = ta.buffer;
  buffer.transfer();
  const e = ta[0];
  const n = ta.length;
  console.log(e, n);
}

// Array.prototype pollution and iteration.
function arrays(): void {
  (Array.prototype as any)[1] = "hole";
  const holey = [1, , 3];
  const h = holey[1];
  console.log(h);
  for (const v of holey) {
    console.log(v);
  }
}

// thisArg of an iterating builtin.
function thisArg(): void {
  [1].forEach(function (this: { k: string }) {
    const t = this;
    console.log(t);
  }, { k: "s" });
}

// An instance made by Object.create.
class Made {
  x = 1;
  get(): number {
    const v = this.x;
    return v;
  }
}

export function main(): void {
  const d = new Derived();
  const dx = d.x;
  console.log(dx);
  detach();
  arrays();
  thisArg();
  const o = Object.create(Made.prototype);
  o.get();
  new Made().get();
}
