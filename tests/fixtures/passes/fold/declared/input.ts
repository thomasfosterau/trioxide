// Negative cases: exported functions in the library world are called with anything their
// types allow, so their parameters are only Declared and nothing folds.

export function twice(n: 5): number {
  return n * 2;
}

export function pick(flag: true): string {
  return flag ? "yes" : "no";
}

export function main(): void {
  console.log(twice(5), pick(true));
}
