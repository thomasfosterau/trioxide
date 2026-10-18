// trustTypes doesn't make an open call site complete: what runs at `shape.area()` is still
// unknown to the analysis (a Proven fact), so the call stays here too.
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
