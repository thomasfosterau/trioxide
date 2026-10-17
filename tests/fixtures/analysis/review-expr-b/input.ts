// Array lengths changed inside a bounded loop.
function truncate(): number {
  const a = [1, 2, 3];
  let sum = 0;
  for (let i = 0; i < a.length; i++) {
    a.length = 0;
    const v = a[i];
    sum += v;
  }
  return sum;
}

class Popper {
  constructor(private readonly arr: number[]) {}
  get top(): number {
    this.arr.pop();
    return 0;
  }
}

function viaGetter(): number {
  const a = [1, 2, 3];
  const p = new Popper(a);
  let sum = 0;
  for (let i = 0; i < a.length; i++) {
    const t = p.top;
    const v = a[i];
    sum += v + t;
  }
  return sum;
}

function viaValueOf(): string {
  const a = [1, 2, 3];
  const o = {
    valueOf(): number {
      a.pop();
      return 1;
    },
  };
  let s = "";
  for (let i = 0; i < a.length; i++) {
    const n = +o;
    const v = a[i];
    s += `${v}${n}`;
  }
  return s;
}

export function main(): void {
  console.log(truncate(), viaGetter(), viaValueOf());
}
