// `norm` runs before `Vec2` is declared, which throws in JS.
function norm(x: number, y: number): number {
  "use wasm";
  const v = new Vec2(x, y);
  return Math.sqrt(v.x * v.x + v.y * v.y);
}

let early = -1;
try {
  early = norm(3, 4);
} catch {
  early = -2;
}

class Vec2 {
  x: number;
  y: number;
  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
  }
}

export function main(): void {
  console.log(early, norm(6, 8));
}
