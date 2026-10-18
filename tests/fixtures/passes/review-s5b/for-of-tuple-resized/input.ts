// Review: TypeScript lets code push onto a tuple, so a tuple's spread can't be written out
// from its type.

export function count(pair: [number, number]): number {
  pair.push(5);
  return countArgs(...pair);
}

function countArgs(...values: number[]): number {
  return values.length;
}

export function main(): void {
  console.log(count([1, 2]));
}
