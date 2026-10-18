// Review: a `const` in a loop head (the loop ends some other way).

function run(): number {
  const items: number[] = [];
  for (let k = 4; k <= 6; k++) items.push(k);
  let sum = 0;
  for (const step = 2; sum < 100; ) {
    for (let i = 0; i < items.length; i++) sum += items[i] * step;
  }
  return sum;
}

export function main(): void {
  console.log(run());
}
