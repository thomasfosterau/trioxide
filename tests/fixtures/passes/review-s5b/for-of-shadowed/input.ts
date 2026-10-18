// Review: the loop body may declare the array's name, or `i`, itself.

function run(): number[] {
  const xs = [1, 2, 3];
  const out: number[] = [];
  for (const x of xs) {
    const xs = [x];
    out.push(xs[0]);
  }
  for (const y of xs) {
    const i = y * 10;
    out.push(i);
  }
  return out;
}

export function main(): void {
  console.log(run());
}
