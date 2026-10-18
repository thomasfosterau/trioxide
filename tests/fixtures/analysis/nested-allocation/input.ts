// An expression allocates when any part of it does: `wrap([seed])` makes a new array on
// every iteration, so it is not loop-invariant even though `wrap` itself is pure.
function wrap(values: number[]): number[] {
  return values;
}

export function main(): void {
  const seed = 1;
  const made: number[][] = [];
  for (let i = 0; i < 3; i++) {
    const values = wrap([seed]);
    made.push(values);
  }
  let total = 0;
  while (total < 3) {
    const pair = [total, seed].length;
    total += pair;
  }
  console.log(made[0] === made[1], made.length, total);
}
