// Review: destructuring a `null` element must throw the error it threw before.

function viaCallback(): string {
  const rows: ([number, number] | null)[] = [[1, 2], null];
  try {
    const sums = rows.map(([a, b]: any) => a + b);
    return String(sums);
  } catch (error) {
    return (error as Error).message;
  }
}

function viaLoop(): string {
  const rows: ({ a: number } | null)[] = [{ a: 1 }, null];
  let total = 0;
  try {
    for (const { a } of rows as { a: number }[]) total += a;
    return String(total);
  } catch (error) {
    return (error as Error).message;
  }
}

export function main(): void {
  console.log(viaCallback());
  console.log(viaLoop());
}
