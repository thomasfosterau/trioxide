// `for…of` over arrays (PLAN §4 pass 7) in hot loops, and array spreads.

const grid: number[][] = [];
for (let y = 0; y < 200; y++) {
  const row: number[] = [];
  for (let x = 0; x < 200; x++) row.push((x * y) % 17);
  grid.push(row);
}

const weights: number[] = [];
for (let i = 0; i < 64; i++) weights.push(i % 5);

function total(): number {
  let sum = 0;
  for (const row of grid) {
    for (const cell of row) sum += cell;
  }
  return sum;
}

function weighted(): number {
  let sum = 0;
  for (let round = 0; round < 50; round++) {
    const copy = [...weights];
    for (const w of copy) sum += w * round;
  }
  return sum;
}

export function bench(): number {
  return total() + weighted();
}

export function main(): void {
  console.log(bench());
}
