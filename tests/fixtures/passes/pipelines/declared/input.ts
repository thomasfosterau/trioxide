// Negative cases: in the library world an exported function's parameters are only Declared,
// so nothing proves the array is a plain one (it could be a subclass or a proxy).

export function total(values: number[]): number {
  return values.map((x) => x * 2).reduce((s, x) => s + x, 0);
}

export function names(people: { name: string }[]): string[] {
  return people.map((p) => p.name);
}

export function main(): void {
  console.log(total([1, 2, 3]), names([{ name: "a" }]));
}
