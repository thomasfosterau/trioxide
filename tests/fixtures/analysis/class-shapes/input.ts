// Hidden-class facts: fields set up front keep one shape; fields added later or deleted don't.

class Point {
  x: number;
  y: number;
  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
  }
  norm(): number {
    return Math.sqrt(this.x /*@proven number*/ * this.x + this.y * this.y);
  }
}

class Grows {
  a = 1;
  constructor() {
    this.setup();
  }
  setup(): void {
    // Runs while the instance is being built.
    console.log(this.a /*@proven number int*/);
  }
  extend(): void {
    (this as any).b = 2;
  }
}

class Shrinks {
  value: number | undefined = 1;
  clear(): void {
    delete this.value;
  }
}

class WithParams {
  constructor(public readonly id: number, private label: string) {}
  describe(): string {
    return `${this.id /*@proven number int*/}:${this.label /*@proven string*/}`;
  }
}

export function main(): void {
  const p = new Point(3, 4);
  console.log(p.norm(), p.x /*@proven number int*/);
  const g = new Grows();
  g.extend();
  const s = new Shrinks();
  s.clear();
  console.log(s.value /*@proven union*/);
  console.log(new WithParams(7, "seven").describe());
}
