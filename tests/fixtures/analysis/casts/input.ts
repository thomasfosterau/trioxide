// Casts and non-null assertions change what TypeScript believes, never the value.

declare function lookup(key: string): string | undefined;

function width(value: unknown): number {
  // Every caller is known (app mode) and passes 5, so the cast is irrelevant here.
  return (value as number) /*@proven number int*/ * 2;
}

export function main(): void {
  const s = "42";
  const n = s as unknown as number;
  console.log(n /*@proven string*/);
  const angle = <number>(<unknown>"3");
  console.log(angle /*@proven string*/);

  const maybe: string | undefined = Math.random() > 2 ? "x" : undefined;
  // `!` does not remove undefined at run time.
  console.log(maybe! /*@proven union*/);

  const point = { x: 1 } satisfies { x: number };
  console.log(point.x /*@proven number int*/);
  console.log(width(5));
  console.log(typeof lookup);
}
