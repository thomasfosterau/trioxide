// Positive cases: values, ranges and branches the facts decide.

function wrap(): number {
  // int32 wraparound.
  return (2147483647 + 1) | 0;
}

function signs(): number[] {
  const zero = 0;
  // -0 must come out as -0.
  return [zero * -1, -zero, zero - 0];
}

function bounded(): string {
  const r = Math.random();
  // r is in [0, 1): the comparisons are decided by its range.
  if (r > 2) {
    return "impossible";
  }
  const label = r < 1 ? "below one" : "one or more";
  return r >= 0 && label.length > 0 ? label : "negative";
}

function rounding(): number[] {
  const i = Math.floor(Math.random() * 10);
  // i is an integer: these are i itself.
  const big = Math.max(i + 100, i);
  return [Math.floor(i) - i, Math.round(i) - i, Math.abs(big) - big, Math.min(i, i + 1) - i];
}

function guard(n: number): string {
  // Every caller passes 4.
  if (n > 3) {
    return "big";
  }
  return "small";
}

export function main(): void {
  console.log(wrap(), signs(), bounded(), rounding(), guard(4));
  console.log(Object.is(signs()[0], -0), 1 / signs()[1]);
}
