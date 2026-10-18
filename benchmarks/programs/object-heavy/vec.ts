// Small immutable vectors and particles, the way object-oriented simulation code is written.

export class Vec2 {
  constructor(
    readonly x: number,
    readonly y: number,
  ) {}

  add(other: Vec2): Vec2 {
    return new Vec2(this.x + other.x, this.y + other.y);
  }

  sub(other: Vec2): Vec2 {
    return new Vec2(this.x - other.x, this.y - other.y);
  }

  scale(factor: number): Vec2 {
    return new Vec2(this.x * factor, this.y * factor);
  }

  dot(other: Vec2): number {
    return this.x * other.x + this.y * other.y;
  }

  length(): number {
    return Math.sqrt(this.dot(this));
  }
}

export class Particle {
  constructor(
    public position: Vec2,
    public velocity: Vec2,
    readonly mass: number,
  ) {}

  kinetic(): number {
    return 0.5 * this.mass * this.velocity.dot(this.velocity);
  }
}

export interface Box {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}
