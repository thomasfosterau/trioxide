// `new Array(n).fill(v)` written and read at indices whose proven range is inside its length:
// the array stays packed, and its elements stay numbers. The keys come from records another
// module builds, so in early sweeps they have no value yet, which must not count as "any key".
import { makeRows, type Row } from "./records.js";

function totals(rows: Row[]): number[] {
  const sums = new Array<number>(8).fill(0);
  rows.forEach((row) => {
    sums[row.key] += row.amount;
  });
  return sums;
}

function counts(rows: Row[]): number[] {
  const seen = new Array<number>(8).fill(0);
  for (const row of rows) seen[row.key]++;
  return seen;
}

function spill(rows: Row[]): number {
  // Indices up to 8, one past the end: a hole may be read, and a write grows the array.
  const xs = new Array<number>(8).fill(1);
  let s = 0;
  for (const row of rows) s += xs[row.key + 1] ?? 100;
  xs[rows.length % 9] = 2;
  return s + xs.length;
}

const ROWS = makeRows(200);

export function main(): void {
  const rows = ROWS;
  const sums = totals(rows);
  console.log(sums.map((v) => v.toFixed(1)).join(), counts(rows).join(), spill(rows));
  const first = sums[rows[0]!.key]!;
  console.log(first.toFixed(1));
}
