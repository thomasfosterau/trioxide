// Adversarial: callbacks that change the array they iterate. The range is fixed when the
// call starts, elements are read at each step, and deleted elements are skipped.

function pushes(): number[] {
  const a = [1, 2, 3];
  const seen: number[] = [];
  a.forEach((x) => {
    seen.push(x);
    a.push(x * 10);
  });
  return seen.concat(a);
}

function pops(): number[] {
  const a = [1, 2, 3, 4];
  const seen: number[] = [];
  a.forEach((x) => {
    seen.push(x);
    a.pop();
  });
  return seen;
}

function rewrites(): number[] {
  const a = [1, 2, 3];
  return a.map((x, i) => {
    if (i + 1 < a.length) a[i + 1] = a[i + 1] * 100;
    return x;
  }).map((v) => v);
}

function deletes(): string {
  const a = [1, 2, 3, 4];
  let out = "";
  a.forEach((x, i) => {
    out += x;
    delete a[i + 1];
  });
  return out;
}

function shrinksDuringFind(): number | undefined {
  const a = [1, 2, 3, 4];
  return a.find((x) => {
    a.length = 2;
    return x === 4;
  });
}

export function main(): void {
  console.log(pushes(), pops(), rewrites(), deletes(), shrinksDuringFind());
}
