export interface Point {
  x: number;
  y: number;
}

export const UNIT: Point = { x: 1, y: 1 };

export function distance(a: Point, b: Point): number {
  return Math.hypot(b.x - a.x, b.y - a.y);
}
