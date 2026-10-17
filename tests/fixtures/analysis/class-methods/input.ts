// Calls to methods of known classes resolve through the prototype chain, `super` included.

class Shape {
  area(): number {
    return 0;
  }
  describe(): string {
    return "area " + this.area();
  }
}

class Square extends Shape {
  constructor(private side: number) {
    super();
  }
  override area(): number {
    return this.side * this.side;
  }
}

class Circle extends Shape {
  constructor(private radius: number) {
    super();
  }
  override area(): number {
    return Math.PI * this.radius ** 2;
  }
  override describe(): string {
    return "circle, " + super.describe();
  }
}

function total(shapes: Shape[]): number {
  let sum = 0;
  for (const shape of shapes) {
    sum += shape.area();
  }
  return sum;
}

export function main(): void {
  const square = new Square(2);
  console.log(square.area() /*@proven number int*/);
  const shapes = [square, new Circle(1)];
  console.log(total(shapes) /*@proven number*/, shapes[1].describe());
  const area = square.area;
  // A detached method: `this` is undefined, and that's a TypeError at run time.
  try {
    area();
  } catch {
    console.log("detached");
  }
}
