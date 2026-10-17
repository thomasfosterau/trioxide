// `fill`: stores only its value; `new Array(n).fill(v)` has no holes, a partial fill may.

function full(n: number): number {
  const a = new Array(n).fill(1);
  let total = 0;
  for (let i = 0; i < a.length; i++) {
    total += (a[i] /*@proven number int in-bounds*/);
  }
  return total;
}

function partial(n: number): number {
  const a: number[] = new Array(n).fill(2, 1);
  let total = 0;
  for (let i = 0; i < a.length; i++) {
    total += a[i] ?? 100;
  }
  return total;
}

function refill(values: number[]): number[] {
  return values.fill(7, 0, 2);
}

export function main(): void {
  console.log(full(0), full(3), partial(0), partial(3), refill([1, 2, 3]).join(","));
}
