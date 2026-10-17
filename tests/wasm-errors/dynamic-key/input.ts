// Objects are plain-data structs with fixed fields: no computed keys.
interface Point {
  x: number;
  y: number;
}

function coordinate(p: Point, axis: number): number {
  "use wasm";
  return p[axis === 0 ? "x" : "y"];
}

export function main(): void {
  console.log(coordinate({ x: 1, y: 2 }, 0));
}
