"use wasm";
// Plain-data objects across the boundary (read, written, returned) and a class used only
// inside the island.

export interface Particle {
  x: number;
  y: number;
  vx: number;
  vy: number;
  alive: boolean;
}

class Vec {
  x: number;
  y: number;

  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
  }

  plus(other: Vec): Vec {
    return new Vec(this.x + other.x, this.y + other.y);
  }

  dot(other: Vec): number {
    return this.x * other.x + this.y * other.y;
  }
}

export function move(p: Particle, dt: number): void {
  if (!p.alive) return;
  p.x += p.vx * dt;
  p.y += p.vy * dt;
  if (p.x * p.x + p.y * p.y > 100) p.alive = false;
}

export function speed(p: Particle): number {
  const v = new Vec(p.vx, p.vy);
  return Math.sqrt(v.dot(v));
}

export function bounds(xs: Float64Array): { lo: number; hi: number; spread: number } {
  let lo = Infinity;
  let hi = -Infinity;
  for (const x of xs) {
    lo = Math.min(lo, x);
    hi = Math.max(hi, x);
  }
  return { lo, hi, spread: hi - lo };
}

export function sum(xs: Float64Array, ys: Float64Array): { x: number; y: number } {
  let acc = new Vec(0, 0);
  for (let i = 0; i < xs.length; i++) acc = acc.plus(new Vec(xs[i], ys[i]));
  return { x: acc.x, y: acc.y };
}
