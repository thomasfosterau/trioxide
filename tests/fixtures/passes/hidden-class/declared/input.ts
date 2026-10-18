// Negative cases: in the library world exported classes' instances reach unknown code, which
// could enumerate their keys.

export class Point {
  x: number;
  constructor(x: number) {
    this.x = x;
  }
}

export function make(): Point {
  const p = new Point(1) as Point & { y?: number };
  p.y = 2;
  return p;
}

export function main(): void {
  console.log(make());
}
