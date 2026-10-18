// Negative cases: in the library world an exported function's parameters are only Declared:
// the array could be a subclass or a proxy with its own iterator.

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
