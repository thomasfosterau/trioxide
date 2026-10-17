// Module-private functions driven with many inputs, so the runtime check sees every fact
// under more than one value (parameters, narrowing, ranges, bounds, shapes and effects).

class Vec2 {
  constructor(
    public x: number,
    public y: number,
  ) {}
  length(): number {
    return Math.sqrt(this.x * this.x + this.y * this.y);
  }
}

function classify(value: number | string | null | undefined): string {
  if (value === null || value === undefined) {
    return "none";
  }
  if (typeof value === "string") {
    return value.length > 3 ? "long" : "short";
  }
  if (Number.isNaN(value)) {
    return "nan";
  }
  return value < 0 ? "negative" : value === 0 ? (Object.is(value, -0) ? "-0" : "0") : "positive";
}

function clamp(value: number, low: number, high: number): number {
  return Math.min(Math.max(value, low), high);
}

function sum(values: number[]): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    total += values[i];
  }
  return total;
}

function bytes(n: number): number {
  const out = new Uint8Array(n);
  for (let i = 0; i < out.length; i++) {
    out[i] = i * 37;
  }
  return out.length === 0 ? -1 : out[out.length - 1];
}

function shape(flag: boolean): number {
  const point = { x: 1, y: 2 };
  const v = new Vec2(point.x, flag ? point.y : -point.y);
  return v.length() + point.x;
}

function divide(a: number, b: number): number {
  return (a / b) | 0;
}

export function main(): void {
  const inputs: (number | string | null | undefined)[] = [
    0, -0, 1, -1, 0.5, NaN, Infinity, -Infinity, 2 ** 31, -(2 ** 31) - 1, "", "abcd", null, undefined,
  ];
  const seen: string[] = [];
  for (const input of inputs) {
    seen.push(classify(input));
  }
  const numbers = [0, -0, 1, -1, 0.5, NaN, Infinity, -Infinity, 1e300, -1e-300];
  let acc = 0;
  for (const a of numbers) {
    for (const b of numbers) {
      acc += clamp(a, -1, 1) + divide(a, b);
    }
  }
  console.log(seen.join(","), acc);
  console.log(sum([]), sum([1]), sum([1, 2.5, -3]), sum(numbers.slice(0, 3)));
  console.log(bytes(0), bytes(1), bytes(10), bytes(300));
  console.log(shape(true), shape(false));
}
