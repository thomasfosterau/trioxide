// Negative case: in the library world `shape` may be anything a caller passes, and only
// TypeScript says `shape.area()` runs `Rect.area`. The analysis records that as a Declared edge
// on an open call site, so the call stays and the skip is logged.
export class Rect {
  constructor(
    public w: number,
    public h: number,
  ) {}

  area(): number {
    return this.w * this.h;
  }
}

export function measure(shape: Rect): number {
  return shape.area() + 1;
}

export function main(): void {
  console.log(measure(new Rect(2, 3)));
}
