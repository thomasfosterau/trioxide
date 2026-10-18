// Even with trustTypes, the analysis doesn't model builtin calls on Declared receivers (their
// callbacks escape to unknown code), so the chains stay.

export function total(values: number[]): number {
  return values.map((x) => x * 2).reduce((s, x) => s + x, 0);
}

export function names(people: { name: string }[]): string[] {
  return people.map((p) => p.name);
}

export function main(): void {
  console.log(total([1, 2, 3]), names([{ name: "a" }]));
}
