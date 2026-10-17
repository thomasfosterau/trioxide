export function lengthThenPop(): number {
  const a = [1, 2, 3];
  const n = a.length;
  a.pop();
  let sum = 0;
  for (let i = 0; i < n; i++) {
    const v = a[i];
    sum += v === undefined ? 100 : v;
  }
  return sum;
}

export function lengthAssign(): number {
  const a = [1, 2, 3];
  let sum = 0;
  for (let i = 0; i < a.length; i++) {
    a.length = 1;
    const v = a[i];
    sum += v === undefined ? 100 : v;
  }
  return sum;
}

function shrink(b: number[]): void {
  b.length = 0;
}

export function aliasShrink(): number {
  const a = [1, 2, 3];
  const n = a.length;
  shrink(a);
  let sum = 0;
  for (let i = 0; i < n; i++) {
    const v = a[i];
    sum += v === undefined ? 100 : v;
  }
  return sum;
}

export function main(): void {
  console.log(lengthThenPop(), lengthAssign(), aliasShrink());
}
