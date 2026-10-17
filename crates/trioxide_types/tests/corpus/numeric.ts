// Numeric kernels: the code WASM islands are made of.

export function dot(a: Float64Array, b: Float64Array): number {
  let sum = 0;
  for (let i = 0; i < a.length; i++) {
    sum += a[i] * b[i];
  }
  return sum;
}

export function blur(src: Uint8ClampedArray, dst: Uint8ClampedArray, width: number, height: number): void {
  for (let y = 1; y < height - 1; y++) {
    for (let x = 1; x < width - 1; x++) {
      const i = (y * width + x) * 4;
      for (let c = 0; c < 3; c++) {
        const total =
          src[i + c - 4] + src[i + c + 4] + src[i + c - width * 4] + src[i + c + width * 4] + src[i + c];
        dst[i + c] = (total / 5) | 0;
      }
      dst[i + 3] = 255;
    }
  }
}

export class Vec2 {
  constructor(public x: number, public y: number) {}
  add(other: Vec2): Vec2 {
    return new Vec2(this.x + other.x, this.y + other.y);
  }
  scale(factor: number): Vec2 {
    return new Vec2(this.x * factor, this.y * factor);
  }
  get length(): number {
    return Math.sqrt(this.x * this.x + this.y * this.y);
  }
}

export function centroid(points: Vec2[]): Vec2 {
  const total = points.reduce((acc, p) => acc.add(p), new Vec2(0, 0));
  return total.scale(1 / Math.max(points.length, 1));
}

export function histogram(values: number[], buckets: number): Int32Array {
  const counts = new Int32Array(buckets);
  const min = Math.min(...values);
  const max = Math.max(...values);
  const width = (max - min) / buckets || 1;
  for (const value of values) {
    const index = Math.min(buckets - 1, Math.floor((value - min) / width));
    counts[index]++;
  }
  return counts;
}

export function mandelbrot(cx: number, cy: number, limit = 255): number {
  let zx = 0;
  let zy = 0;
  let n = 0;
  while (zx * zx + zy * zy < 4 && n < limit) {
    const t = zx * zx - zy * zy + cx;
    zy = 2 * zx * zy + cy;
    zx = t;
    n++;
  }
  return n;
}

export function hash(text: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h;
}

export function primes(limit: number): number[] {
  const sieve = new Uint8Array(limit + 1);
  const out: number[] = [];
  for (let i = 2; i <= limit; i++) {
    if (sieve[i]) continue;
    out.push(i);
    for (let j = i * i; j <= limit; j += i) sieve[j] = 1;
  }
  return out;
}

export function matmul(a: number[][], b: number[][]): number[][] {
  return a.map((row) => b[0].map((_, j) => row.reduce((acc, value, k) => acc + value * b[k][j], 0)));
}

export const table = Array.from({ length: 16 }, (_, i) => Math.round(Math.sin(i / 16) * 1000) / 1000);
export const clamp = (value: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, value));
