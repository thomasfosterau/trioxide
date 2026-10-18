// Positive cases: `for…of` over arrays the analysis made becomes an indexed loop, and spreads
// of arrays without holes become `slice`/`concat`.

function sum(): number {
  const values = [1, 2, 3, 4];
  let total = 0;
  for (const value of values) total += value;
  return total;
}

function heads(): string {
  const pairs: [string, number][] = [
    ["a", 1],
    ["b", 2],
  ];
  let out = "";
  // Destructuring, `let`, `var` and an existing binding as the target.
  for (const [key, count] of pairs) out += key.repeat(count);
  for (let [key] of pairs) {
    key += "!";
    out += key;
  }
  for (var entry of pairs) out += entry[1];
  let last = "";
  for (last of ["x", "y"]) out += last;
  return `${out}:${entry![0]}:${last}`;
}

function control(): string {
  const grid = [
    [1, 2, 3],
    [4, 5, 6],
    [7, 8, 9],
  ];
  let out = "";
  rows: for (const row of grid) {
    for (const cell of row) {
      if (cell === 5) continue rows;
      if (cell === 8) break rows;
      if (cell % 2 === 0) continue;
      out += cell;
    }
  }
  return out;
}

function closures(): number[] {
  const makers: (() => number)[] = [];
  // Each iteration has its own binding.
  for (const value of [10, 20, 30]) makers.push(() => value);
  return makers.map((make) => make());
}

function fromCall(): number {
  let total = 0;
  for (const value of digits()) total = total * 10 + value;
  return total;
}

function digits(): number[] {
  return [4, 2];
}

function spreads(): string {
  const a = [1, 2];
  const b = [3];
  const copy = [...a];
  copy.push(9);
  const joined = [...a, ...b];
  return `${a.join()} ${copy.join()} ${joined.join()}`;
}

export function main(): void {
  console.log(sum(), heads(), control(), closures(), fromCall(), spreads());
}
