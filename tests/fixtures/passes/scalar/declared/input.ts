// Scalar replacement rests only on escape and points-to facts, which are always Proven: here
// the point is handed to `report`, which library callers may replace, so it stays an object.
export function report(p: { x: number; y: number }): number {
  return p.x + p.y;
}

export function main(): void {
  const p = { x: 1, y: 2 };
  const q = { x: 3, y: 4 };
  console.log(report(p), q.x * q.y);
}
