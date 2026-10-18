// Adversarial: a program that assigns the array iterator. Reaching `Array.prototype` turns the
// builtin models off, so no loop is lowered.

Array.prototype[Symbol.iterator] = Array.prototype.values;

function sum(): number {
  let total = 0;
  for (const value of [1, 2, 3]) total += value;
  return total;
}

export function main(): void {
  console.log(sum(), [...[4, 5]]);
}
