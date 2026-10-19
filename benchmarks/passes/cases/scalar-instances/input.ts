// Pass 8 (scalar replacement) on class instances: `new Point(x, y)` temporaries that never
// leave the loop body.
class Point {
  x: number;
  y: number;
  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
  }
}

export function bench(): number {
  let total = 0;
  for (let i = 0; i < 4096; i++) {
    const p = new Point(i * 0.5, i * 0.25);
    const q = new Point(p.y - 1, p.x + 1);
    p.x += q.x;
    p.y -= q.y;
    total += p.x * q.y - p.y * q.x;
  }
  return total;
}
