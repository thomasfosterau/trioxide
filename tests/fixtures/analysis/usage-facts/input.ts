// Call-site completeness, expression and loop effects, points-to and reachability.

let log = 0;

function bump(n: number): number {
  log += n;
  return n;
}

function square(x: number): number {
  return x * x;
}

function never(): number {
  return 1;
}

function early(flag: boolean): number {
  if (flag) {
    return 1;
  }
  return 2;
  log = 99;
}

function loop(values: number[]): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    total += values[i] * values[i];
  }
  return total;
}

function popping(values: number[]): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    values.pop();
    total += i;
  }
  return total;
}

function invariant(values: number[], k: number): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    total += values[i] + k * 3;
  }
  return total;
}

export function main(): void {
  const point = { x: 1 };
  const pure = square(3);
  const effect = bump(2);
  const outside = (globalThis as any).unknown?.(1);
  console.log(point.x, pure, effect, outside, early(true), early(false));
  console.log(loop([1, 2, 3]), popping([1, 2, 3]), invariant([1, 2], 4), invariant([], 5));
}
