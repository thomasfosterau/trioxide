// world: library
// Library mode: exported functions may be called by anyone with anything their types allow.

export function scale(value: number, factor: number): number {
  // Parameters of an export rest on the declared types.
  return value /*@declared number*/ * factor;
}

function helper(value: number): number {
  // Only called below with a literal: all call sites are known.
  return value /*@proven number int*/ + 1;
}

export const shared = { count: 0 };

export function bump(): number {
  shared.count++;
  // Importers may have changed `shared`.
  return shared.count /*@declared number*/;
}

export function main(): void {
  console.log(scale(2, 3), helper(41), bump());
}
