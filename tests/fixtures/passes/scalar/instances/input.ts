// Instances of classes whose constructor only assigns fields become locals.
class Vec2 {
  x: number;
  y: number;
  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
  }
  length(): number {
    return Math.hypot(this.x, this.y);
  }
}

class Particle {
  mass = 1;
  alive = true;
  label = "p";
  charge = -2;
  speed: number;
  constructor(speed: number) {
    this.speed = speed;
  }
}

class Empty {}

let calls = 0;
function next(): number {
  calls++;
  return calls;
}

export function main(): void {
  let total = 0;
  for (let i = 0; i < 10; i++) {
    const v = new Vec2(i, i * 2);
    v.x += 1;
    v.y++;
    total += v.x * v.y;
    // The arguments run in their order, once.
    const w = new Vec2(next(), next() * 10);
    total += w.y - w.x;
    // Missing arguments are undefined.
    const u = new Vec2(i);
    total += u.y === undefined ? 1 : 0;
    // A pure argument the constructor ignores is dropped.
    const p = new Particle(i, 99);
    p.mass += p.speed;
    total += p.mass + p.charge + (p.alive ? p.label.length : 0);
    // Not replaced: a method is called.
    const m = new Vec2(3, 4);
    total += m.length();
    // Not replaced: no keys, and the object itself is used.
    const e = new Empty();
    total += e instanceof Empty ? 1 : 0;
  }
  console.log(total, calls);
}
