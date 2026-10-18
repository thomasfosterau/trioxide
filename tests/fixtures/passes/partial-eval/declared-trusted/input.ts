// With trustTypes, the declared parameter type is proof: the call is evaluated.
export function scale(x: number): number {
  return x * 2 + 1;
}

export const SCALED = scale(20);

export function main(): void {
  console.log(SCALED);
}
