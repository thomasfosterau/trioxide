// A chain that is one `map` stays the builtin call (nothing to fuse); a `map` feeding another
// stage still becomes a loop.
const rows: number[][] = [];
for (let i = 0; i < 8; i++) rows.push([i, i + 1, i + 2, i + 3]);

function doubled(row: number[]): number[] {
  return row.map((v) => v * 2);
}

function labels(row: number[]): string[] {
  const out = row.map((v, i) => `${i}:${v}`);
  return out;
}

function sumOfSquares(row: number[]): number {
  return row.map((v) => v * v).reduce((a, b) => a + b, 0);
}

function evensDoubled(row: number[]): number[] {
  return row.filter((v) => v % 2 === 0).map((v) => v * 2);
}

export function main(): void {
  let total = 0;
  for (const row of rows) {
    const d = doubled(row);
    total += d[3]! + sumOfSquares(row) + evensDoubled(row).length;
    console.log(labels(row).join(" "));
  }
  console.log(total, doubled([]).length, [1, , 3].map((v) => v * 2).length);
}
