// The negative cases with trustTypes: declared facts count as proof, so `n * 2` folds.

export function twice(n: 5): number {
  return n * 2;
}

export function pick(flag: true): string {
  return flag ? "yes" : "no";
}

export function main(): void {
  console.log(twice(5), pick(true));
}
