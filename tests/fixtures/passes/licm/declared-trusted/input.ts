// Even with trustTypes, reading an element of an array the analysis doesn't follow may run
// unknown code (a getter) that resizes arrays, so the length is still read every time.

export function sum(values: number[]): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) total += values[i];
  return total;
}

export function main(): void {
  console.log(sum([1, 2, 3]));
}
