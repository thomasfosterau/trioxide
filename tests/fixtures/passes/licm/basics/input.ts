// Positive cases: invariant work moves out of loops, under readable names.

function build(n: number): number[] {
  const values: number[] = [];
  for (let i = 0; i < n; i++) values.push(i * i);
  return values;
}

function scale(factor: number): string {
  const values = build(4);
  const out: number[] = [];
  // `values.length` (the loop resizes `out`, not `values`) and `Math.sqrt(factor)` move into
  // the loop's head.
  for (let i = 0; i < values.length; i++) {
    out.push(values[i] * Math.sqrt(factor));
  }
  return out.join();
}

function countdown(): string {
  const labels = build(3).map((n) => `#${n}`);
  let i = labels.length;
  let out = "";
  // A `while` test: hoisted right before the loop.
  while (i > labels.length - 3) {
    i--;
    out += labels[i] + " ";
  }
  return out;
}

function atLeastOnce(limit: number): number {
  const steps = build(limit);
  let total = 0;
  let k = 0;
  // A `do…while` body: `steps.length` stays, as only expressions that cannot throw leave a body.
  do {
    total += steps.length;
    k++;
  } while (k < 3);
  return total;
}

function nested(): number {
  const rows = [build(2), build(3)];
  const weights = build(5);
  let total = 0;
  // `weights.length` is invariant in both loops: it moves out of the outer one.
  rows: for (let r = 0; r < rows.length; r++) {
    for (let c = 0; c < weights.length; c++) {
      if (c === 4) continue rows;
      total += weights[c] * r;
    }
  }
  return total;
}

class Grid {
  width = 3;
  height = 2;
  cells: number[] = [];

  fill(): number {
    // The loop writes an object (`cells`), so the analysis can't tell `this.width` stays the
    // same: it stays in the test.
    for (let y = 0; y < this.height; y++) {
      for (let x = 0; x < this.width; x++) this.cells.push(x * y);
    }
    return this.cells.length;
  }
}

function grid(): number {
  const grid = new Grid();
  // Leaves the height's range open: with a precise bound, the analysis's loop warm start
  // makes `fill`'s facts depend on how units are chunked (an S2 issue).
  grid.height += grid.cells.length;
  return grid.fill();
}

export function main(): void {
  console.log(scale(4), countdown(), atLeastOnce(2), nested(), grid());
}
