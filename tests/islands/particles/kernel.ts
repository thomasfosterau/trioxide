// A plain-data class used inside the island (Vec2 never crosses the boundary), plus
// plain-data structs that do: Body parameters (read and mutated) and a Point result.

class Vec2 {
  x: number;
  y: number;

  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
  }

  add(other: Vec2): Vec2 {
    return new Vec2(this.x + other.x, this.y + other.y);
  }

  scale(k: number): Vec2 {
    return new Vec2(this.x * k, this.y * k);
  }

  length(): number {
    return Math.hypot(this.x, this.y);
  }
}

export interface Body {
  x: number;
  y: number;
  vx: number;
  vy: number;
  mass: number;
  layer: number;
  pinned: boolean;
}

export function centroid(xs: Float64Array, ys: Float64Array): { x: number; y: number } {
  let sum = new Vec2(0, 0);
  const n = Math.min(xs.length, ys.length);
  for (let i = 0; i < n; i++) sum = sum.add(new Vec2(xs[i], ys[i]));
  const c = sum.scale(1 / n);
  return { x: c.x, y: c.y };
}

export function spread(xs: Float64Array, ys: Float64Array): number {
  const c = centroid(xs, ys);
  let widest = 0;
  for (let i = 0; i < xs.length; i++) {
    widest = Math.max(widest, new Vec2(xs[i] - c.x, ys[i] - c.y).length());
  }
  return widest;
}

export function step(body: Body, ax: number, ay: number, dt: number): void {
  if (body.pinned) return;
  body.vx += ax * dt;
  body.vy += ay * dt;
  body.x += body.vx * dt;
  body.y += body.vy * dt;
}

/** Merges the velocities of two touching bodies on the same layer. */
export function collide(a: Body, b: Body): boolean {
  if (a.layer !== b.layer) return false;
  const d = new Vec2(b.x - a.x, b.y - a.y);
  if (d.length() > 1) return false;
  const total = a.mass + b.mass;
  const vx = (a.vx * a.mass + b.vx * b.mass) / total;
  const vy = (a.vy * a.mass + b.vy * b.mass) / total;
  a.vx = vx;
  a.vy = vy;
  b.vx = vx;
  b.vy = vy;
  a.mass += 0.5;
  b.mass += 0.25;
  return true;
}

export function momentum(a: Body, b: Body): number {
  return new Vec2(a.vx * a.mass + b.vx * b.mass, a.vy * a.mass + b.vy * b.mass).length();
}
