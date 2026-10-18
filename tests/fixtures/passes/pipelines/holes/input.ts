// Adversarial: arrays with holes. `map` keeps holes; `filter`, `forEach`, `reduce`, `some`
// and `every` skip them; `find`, `findIndex` and `includes` visit them as `undefined`; and
// `reduce` with no seed throws on an empty range.

function holey(): (number | undefined)[] {
  // eslint-disable-next-line no-sparse-arrays
  const a = [1, , 3];
  return a;
}

function deleted(): number[] {
  const a = [4, 5, 6];
  delete a[1];
  return a;
}

function grown(): number[] {
  const a = [7];
  a.length = 3;
  return a;
}

function report(): void {
  const a = holey();
  const doubled = a.map((x) => (x ?? 0) * 2);
  console.log(doubled, 1 in doubled, doubled.length);
  const kept = a.filter(() => true);
  console.log(kept, kept.length);
  let visits = 0;
  a.forEach(() => {
    visits++;
  });
  console.log(visits);
  const sum = a.reduce((s: number, x) => s + (x ?? 100), 0);
  const count = a.some((x) => x === undefined);
  const every = a.every((x) => x !== undefined);
  console.log(sum, count, every);
  const found = a.findIndex((x) => x === undefined);
  const has = a.includes(undefined);
  const which = a.find((x) => x === undefined);
  console.log(found, has, which);
  // A hole a `map` keeps would reach `find`: this chain stays.
  const passed = a.map((x) => x).find((x) => x === undefined);
  console.log(passed);
  const d = deleted();
  const plusOne = d.map((x) => x + 1);
  const total = d.reduce((s, x) => s + x, 0);
  console.log(plusOne, total);
  const g = grown();
  const tripled = g.map((x) => x * 3);
  const positive = g.filter((x) => x > 0);
  console.log(tripled, positive);
}

function empty(): string {
  const none: number[] = [];
  try {
    return String(none.reduce((a, b) => a + b));
  } catch (error) {
    return (error as Error).message;
  }
}

function allHoles(): string {
  const a: number[] = new Array(3);
  try {
    return String(a.reduce((x, y) => x + y));
  } catch (error) {
    return error instanceof TypeError ? "TypeError" : "other";
  }
}

export function main(): void {
  report();
  console.log(empty(), allHoles());
}
