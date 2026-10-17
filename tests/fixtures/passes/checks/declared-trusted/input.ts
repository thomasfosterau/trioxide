// The negative cases with trustTypes: the declared types count, so the checks fold.

export function read(point: { x: number }, value: number): unknown[] {
  return [point?.x, value ?? 0, typeof value === "number", point == null];
}

export function main(): void {
  console.log(read({ x: 1 }, 2));
}
