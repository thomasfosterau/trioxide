// Review: a `var` in a loop head is function-scoped, so a hoisted name there must be fresh in
// the whole function.

function run(): number {
  const items: number[] = [];
  for (let k = 1; k <= 3; k++) items.push(k);
  // eslint-disable-next-line no-var
  var itemsLength = items[0] * 100;
  let sum = 0;
  // eslint-disable-next-line no-var
  for (var i = 0; i < items.length; i++) sum += items[i];
  return sum + itemsLength + i;
}

export function main(): void {
  console.log(run());
}
