// Negative cases: in the library world `read` may be called with anything, so its parameter's
// type (never nullish, a number) is only Declared and the checks stay.

export function read(point: { x: number }, value: number): unknown[] {
  return [point?.x, value ?? 0, typeof value === "number", point == null];
}

export function main(): void {
  console.log(read({ x: 1 }, 2));
}
