// Bigints: arithmetic stays bigint; mixing with numbers throws.

function mix(a: bigint, b: number): unknown {
  try {
    return (a as unknown as number) + b;
  } catch {
    return "TypeError";
  }
}

export function main(): void {
  const big = 2n ** 64n;
  console.log(big /*@proven bigint*/, (big + 1n) /*@proven bigint*/, typeof big);
  console.log(mix(1n, 1) /*@unknown*/);
  const shifted = 1 << 3;
  console.log(shifted /*@proven number int*/);
}
