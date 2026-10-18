// Pass 5 (function inlining): small vector helpers and methods called in a hot loop.
class Vec2 {
  constructor(
    public x: number,
    public y: number,
  ) {}

  dot(other: Vec2): number {
    return this.x * other.x + this.y * other.y;
  }

  cross(other: Vec2): number {
    return this.x * other.y - this.y * other.x;
  }
}

function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

const points: Vec2[] = [];
for (let i = 0; i < 1024; i++) {
  points.push(new Vec2((i % 17) - 8, (i % 13) - 6));
}

export function bench(): number {
  let total = 0;
  for (let i = 1; i < points.length; i++) {
    const a = points[i - 1];
    const b = points[i];
    const t = clamp(a.dot(b) / 100, 0, 1);
    total += lerp(a.cross(b), b.dot(b), t);
  }
  return total;
}
