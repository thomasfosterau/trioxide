// With `passes.specialize`, a parameter every caller passes the same literal leaves the
// signature and every call; a parameter callers disagree on stays.
function scale(values: number[], factor: number, offset: number): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    total += values[i] * factor + offset;
  }
  return total;
}

// `arguments` sees every argument, so nothing is removed here.
function counted(a: number, b: number): number {
  return a + b + arguments.length;
}

export function main(): void {
  const data = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20];
  console.log(scale(data, 4, 1), scale([1, 2], 4, -1), scale(data.slice(5), 4, 0));
  console.log(counted(1, 2), counted(3, 2));
}
