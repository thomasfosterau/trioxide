// Negative case: `values` comes from the library's callers, so only TypeScript says it is an
// array whose `map` only calls the closure; the closure stays where it is.
export function doubled(values: number[]): number[] {
  return values.map((v) => v * 2);
}

export function main(): void {
  console.log(doubled([1, 2, 3]).join(","));
}
