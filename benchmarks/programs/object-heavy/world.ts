import { type Box, Particle, Vec2 } from "./vec.js";

const GRAVITY = new Vec2(0, -9.81);
const BOUNDS: Box = { minX: -50, minY: 0, maxX: 50, maxY: 100 };

export function makeParticles(count: number): Particle[] {
  const particles: Particle[] = [];
  for (let i = 0; i < count; i++) {
    const angle = (i * 2.399963) % (2 * Math.PI);
    particles.push(new Particle(new Vec2(Math.cos(angle) * 20, 50 + Math.sin(angle) * 20), new Vec2(Math.sin(i), Math.cos(i) * 2), 1 + (i % 3)));
  }
  return particles;
}

function bounce(p: Particle, box: Box): void {
  const { x, y } = p.position;
  if (x < box.minX || x > box.maxX) p.velocity = new Vec2(-p.velocity.x * 0.9, p.velocity.y);
  if (y < box.minY || y > box.maxY) p.velocity = new Vec2(p.velocity.x, -p.velocity.y * 0.9);
  const clamped = { x: Math.min(Math.max(x, box.minX), box.maxX), y: Math.min(Math.max(y, box.minY), box.maxY) };
  p.position = new Vec2(clamped.x, clamped.y);
}

export function step(particles: Particle[], dt: number): void {
  for (const p of particles) {
    const acceleration = GRAVITY.scale(dt);
    p.velocity = p.velocity.add(acceleration);
    p.position = p.position.add(p.velocity.scale(dt));
    bounce(p, BOUNDS);
  }
}

export function centroid(particles: Particle[]): Vec2 {
  let sum = new Vec2(0, 0);
  let mass = 0;
  for (const p of particles) {
    sum = sum.add(p.position.scale(p.mass));
    mass += p.mass;
  }
  return sum.scale(1 / mass);
}

export function energy(particles: Particle[]): number {
  let total = 0;
  for (const p of particles) total += p.kinetic() + p.mass * 9.81 * p.position.y;
  return total;
}

export function spread(particles: Particle[], center: Vec2): number {
  let max = 0;
  for (const p of particles) {
    const d = p.position.sub(center).length();
    if (d > max) max = d;
  }
  return max;
}
