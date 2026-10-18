// With trustTypes, Declared array types count as proof: the loop is indexed and the tuple
// spread is written out. Nothing says the copied array has no holes, so it stays a spread.

export function sum(values: number[]): number {
  let total = 0;
  for (const value of values) total += value;
  return total;
}

export function plot(point: [number, number]): number {
  return Math.hypot(...point);
}

export function copy(values: number[]): number[] {
  return [...values];
}

export function main(): void {
  console.log(sum([1, 2, 3]), plot([3, 4]), copy([5]));
}
