// Negative case: `scale` is exported in the library world, so its parameter is only declared
// a number and `x * 2` might call `valueOf`; only TypeScript says the call is pure.
export function scale(x: number): number {
  return x * 2 + 1;
}

export const SCALED = scale(20);

export function main(): void {
  console.log(SCALED);
}
