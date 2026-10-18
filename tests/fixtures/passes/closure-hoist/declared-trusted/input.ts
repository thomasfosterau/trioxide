// trustTypes doesn't change escape facts (always Proven): the closure handed to a value from
// the library's callers escapes, so it stays.
export function doubled(values: number[]): number[] {
  return values.map((v) => v * 2);
}

export function main(): void {
  console.log(doubled([1, 2, 3]).join(","));
}
