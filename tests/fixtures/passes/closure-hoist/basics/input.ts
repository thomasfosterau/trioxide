// Closures that capture nothing are created once, at the top level.
const SCALE = 3;

function weights(n: number): number {
  let total = 0;
  for (let i = 0; i < n; i++) {
    // Only called: its identity can't be seen.
    const weight = (v: number): number => v * SCALE + 1;
    total += weight(i);
  }
  return total;
}

function doubled(values: number[]): number[] {
  return values.map((v) => v * 2);
}

export function main(): void {
  console.log(weights(5), doubled([1, 2, 3]).join(","));
}
