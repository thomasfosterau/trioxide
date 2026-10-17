abstract class Shape {
  static count = 0;
  readonly id: number;

  constructor(public readonly name: string) {
    this.id = ++Shape.count;
  }

  abstract area(): number;

  describe(): string {
    return `${this.name}#${this.id} area=${this.area().toFixed(2)}`;
  }
}

class Circle extends Shape {
  #radius: number;

  constructor(radius: number) {
    super("circle");
    this.#radius = radius;
  }

  get radius(): number {
    return this.#radius;
  }

  set radius(value: number) {
    if (value < 0) throw new RangeError("negative radius");
    this.#radius = value;
  }

  area(): number {
    return Math.PI * this.#radius ** 2;
  }
}

class Rect extends Shape {
  declare tag: string;
  private sides: [number, number];

  constructor(width: number, height: number) {
    super("rect");
    this.sides = [width, height];
  }

  area(): number {
    return this.sides[0] * this.sides[1];
  }

  static square(side: number): Rect {
    return new Rect(side, side);
  }
}

export function main(): void {
  const shapes: Shape[] = [new Circle(1), new Rect(2, 3), Rect.square(4)];
  for (const shape of shapes) console.log(shape.describe());
  const circle = shapes[0] as Circle;
  circle.radius = 2;
  console.log(circle.radius, circle.describe());
  try {
    circle.radius = -1;
  } catch (error) {
    console.log((error as Error).name, (error as Error).message);
  }
  console.log(Shape.count, Object.keys(shapes[1]).sort().join(","));
}
