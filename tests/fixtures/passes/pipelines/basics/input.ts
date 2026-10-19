// Positive cases: chains over arrays the analysis made, with inlineable callbacks.

function total(): number {
  const values = [1, 2, 3, 4, 5];
  const total = values.map((x) => x * 2).filter((y) => y > 3).reduce((sum, y) => sum + y, 0);
  return total;
}

interface Person {
  name: string;
  age: number;
}

function adults(): string[] {
  const people: Person[] = [
    { name: "Ada", age: 36 },
    { name: "Bo", age: 12 },
    { name: "Cy", age: 50 },
  ];
  // The element passes through the filter under the same name.
  return people.filter((p) => p.age >= 18).map((p) => p.name.toUpperCase());
}

function questions(): (boolean | number | undefined)[] {
  const xs = [5, 8, 13, 21];
  const any = xs.some((x) => x > 20);
  const all = xs.every((x) => x % 2 === 1);
  const first = xs.find((x) => x > 6);
  const at = xs.findIndex((x) => x === 13);
  const has = xs.includes(8);
  return [any, all, first, at, has];
}

function product(): number {
  // No seed: the first element seeds the accumulator.
  const factors = [2, 3, 7];
  return factors.reduce((a, b) => a * b);
}

function indexed(): string[] {
  const letters = ["a", "b", "c"];
  // The index and array parameters of the first callback.
  return letters.map((letter, index, all) => `${letter}${index}/${all.length}`).map((s) => s);
}

function blockBodies(): number[] {
  const values = [1, 2, 3, 4];
  return values
    .map(function (value) {
      const squared = value * value;
      return squared + 1;
    })
    .filter((n) => {
      const odd = n % 2 === 1;
      return odd;
    });
}

function pairs(): number {
  const entries: [string, number][] = [
    ["a", 1],
    ["b", 2],
  ];
  // A destructuring parameter.
  let weighted = entries.map(([key, count]) => key.length * count).reduce((s, n) => s + n, 0);
  weighted += 1;
  return weighted;
}

function visit(): string {
  const log: string[] = [];
  const items = ["x", "y"];
  items.forEach((item, position) => {
    log.push(`${position}:${item}`);
  });
  return log.join(",");
}

export function main(): void {
  console.log(total(), adults(), questions(), product(), indexed());
  console.log(blockBodies(), pairs(), visit());
}
