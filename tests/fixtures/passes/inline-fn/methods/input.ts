// Methods inline with their receiver as `this`.
class Vec2 {
  constructor(
    public x: number,
    public y: number,
  ) {}

  dot(other: Vec2): number {
    return this.x * other.x + this.y * other.y;
  }

  lengthSquared(): number {
    return this.dot(this);
  }
}

function energy(points: Vec2[]): number {
  let total = 0;
  for (let i = 0; i < points.length; i++) {
    const p = points[i];
    total += p.lengthSquared();
  }
  return total;
}

export function main(): void {
  const a = new Vec2(3, 4);
  const b = new Vec2(1, 2);
  console.log(a.dot(b), a.lengthSquared(), energy([a, b]));
}
