import greet, { VERSION, add } from "./math.js";
import * as shapes from "./shapes.js";
import type { Point } from "./shapes.js";
import { type Named, label } from "./named.js";

export { add };
export * from "./shapes.js";

const origin: Point = { x: 0, y: 0 };

export function main(): void {
  console.log(greet("trioxide"), VERSION, add(2, 3));
  console.log(shapes.distance(origin, { x: 3, y: 4 }), shapes.UNIT.x);
  const thing: Named = { name: "widget" };
  console.log(label(thing));
}
