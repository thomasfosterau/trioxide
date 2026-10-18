// Negative cases: in the library world an exported function's array parameter is only
// Declared: its `length` could be a getter.

export function sum(values: number[]): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) total += values[i];
  return total;
}

export function main(): void {
  console.log(sum([1, 2, 3]));
}
