// With `passes.specialize` off (the default), what callers pass is not folded into bodies.
function defaults(a: number, b: number = a * 2): number {
  return a + b;
}

function bounded(arr: number[]): number {
  let total = 0;
  for (let i = 0; i <= arr.length - 1; i++) {
    total += arr[i];
  }
  return total;
}

class Point {
  constructor(
    public x: number,
    public y: number,
  ) {}
  /** The distance from the origin. */
  length(): number {
    return Math.sqrt(this.x * this.x + this.y * this.y);
  }
}

function guard(n: number): string {
  if (n > 3) {
    return "big";
  }
  return "small";
}

export function main(): void {
  const scale = (x: number): number => x * 10;
  console.log(defaults(1), bounded([1, 2]), new Point(3, 4).length(), scale(2.5), guard(4));
}
