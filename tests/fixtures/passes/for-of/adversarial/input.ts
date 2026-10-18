// Adversarial: what an array iterator does, which the indexed loop must match.

function grows(): number[] {
  // The iterator reads `length` at every step: elements pushed during the loop are visited.
  const a = [1, 2];
  const seen: number[] = [];
  for (const x of a) {
    seen.push(x);
    if (a.length < 5) a.push(x * 10);
  }
  return seen;
}

function shrinks(): number[] {
  const a = [1, 2, 3, 4];
  const seen: number[] = [];
  for (const x of a) {
    seen.push(x);
    a.pop();
  }
  return seen;
}

function holes(): string {
  // eslint-disable-next-line no-sparse-arrays
  const a = [1, , 3];
  let out = "";
  for (const x of a) out += String(x) + ",";
  // A spread turns holes into `undefined`, `slice` wouldn't: this one stays a spread.
  const copy = [...a];
  return `${out} ${1 in copy}`;
}

function reassigned(): number[] {
  // The iterator keeps the array it started with.
  let a = [1, 2, 3];
  const seen: number[] = [];
  for (const x of a) {
    seen.push(x);
    a = [7];
  }
  return seen.concat(a);
}

function rewrites(): number[] {
  // Elements are read when reached.
  const a = [1, 2, 3];
  const seen: number[] = [];
  for (const x of a) {
    seen.push(x);
    a[2] = 30;
  }
  return seen;
}

function others(): string {
  // Strings, sets, maps and generators keep their iterators.
  let out = "";
  for (const c of "a😀b") out += `[${c}]`;
  for (const v of new Set([1, 1, 2])) out += v;
  for (const [k, v] of new Map([["k", 1]])) out += k + v;
  for (const v of gen()) out += v;
  return out;
}

function* gen(): Generator<number> {
  yield 7;
  yield 8;
}

function tdz(): string {
  try {
    const x = [1];
    {
      // The right-hand side reads the loop's own binding, in its temporal dead zone.
      // @ts-expect-error: used before its declaration
      for (const x of x) void x;
    }
    return String(x);
  } catch (error) {
    return error instanceof ReferenceError ? "ReferenceError" : "other";
  }
}

export function main(): void {
  console.log(grows(), shrinks(), holes(), reassigned(), rewrites(), others(), tdz());
}
