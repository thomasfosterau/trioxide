// Methods called on member receivers (`p.velocity.add(…)`): the receiver is evaluated once,
// before the arguments, into a local that stands for `this`.
const log: string[] = [];

class Vec {
  constructor(
    readonly x: number,
    readonly y: number,
  ) {}
  add(other: Vec): Vec {
    return new Vec(this.x + other.x, this.y + other.y);
  }
  dot(other: Vec): number {
    return this.x * other.x + this.y * other.y;
  }
  norm(): number {
    return this.dot(this);
  }
  tag(label: string): string {
    return `${label}:${this.x},${this.y}`;
  }
}

class Particle {
  velocity: Vec;
  reads = 0;
  constructor(public position: Vec) {
    this.velocity = new Vec(1, 2);
  }
  get speedVector(): Vec {
    this.reads++;
    log.push("get");
    return this.velocity;
  }
  energy(): number {
    return this.velocity.dot(this.velocity);
  }
}

class Pair {
  constructor(readonly inner: Particle) {}
  innerEnergy(): number {
    return this.inner.energy() + this.inner.velocity.norm();
  }
}

function arg(value: number): Vec {
  log.push(`arg ${value}`);
  return new Vec(value, value);
}

function make(label: string): Particle {
  log.push(`make ${label}`);
  return new Particle(new Vec(0, 0));
}

export function main(): void {
  const p = new Particle(new Vec(3, 4));
  const moved = p.velocity.add(arg(5));
  console.log(moved.x, moved.y, p.velocity.dot(p.position), p.velocity.norm());
  // A getter receiver runs once, before the argument.
  const fast = p.speedVector.add(arg(1));
  console.log(fast.x, p.reads, log.join("|"));
  log.length = 0;
  // A call as the receiver's object: evaluated once, in order.
  const e = make("a").velocity.dot(arg(2));
  console.log(e, log.join("|"));
  log.length = 0;
  const pair = new Pair(p);
  console.log(pair.innerEnergy(), pair.inner.velocity.tag("v"), p.position.add(p.velocity).add(arg(1)).x);
  console.log(log.join("|"));
}
