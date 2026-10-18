function len(x: number, y: number): number {
  return x * x + y * y;
}

function clamp(v: number, lo: number, hi: number): number {
  if (v < lo) return lo;
  if (v > hi) return hi;
  return v;
}

function sum(...xs: number[]): number {
  let s = 0;
  for (const x of xs) s += x;
  return s;
}

function scaled(a: number, b = a * 2): number {
  return a + b;
}

function log(message: string): void {
  console.log("log:", message);
}

export function main(): void {
  let total = 0;
  for (let i = 0; i < 10; i++) {
    total += len(i, i + 1);
    const c = clamp(i * 3, 2, 20);
    total += c;
  }
  log("total " + total);
  console.log(sum(1, 2, 3), scaled(1), scaled(2, 5));
}
