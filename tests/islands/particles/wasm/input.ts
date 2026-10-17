import { type Body, centroid, collide, momentum, spread, step } from "./kernel.js";

function body(x: number, y: number, vx: number, vy: number, layer = 0, pinned = false): Body {
  return { x, y, vx, vy, mass: 1 + x * 0.25, layer, pinned };
}

export function main(): void {
  const xs = new Float64Array([0, 1, 2.5, -4, 3]);
  const ys = new Float64Array([1, -1, 0.5, 2, 7]);
  const c = centroid(xs, ys);
  console.log(c, Object.keys(c), spread(xs, ys));
  console.log(centroid(new Float64Array(0), new Float64Array(0)), centroid(xs, ys.subarray(0, 2)), spread(xs, ys.subarray(0, 2)));

  const a = body(0, 0, 1, 0.5);
  const b = body(0.5, 0.25, -1, 0, 0);
  const pinned = body(3, 3, 1, 1, 0, true);
  const far = body(10, 10, 0, 0, 1);
  for (let i = 0; i < 3; i++) {
    step(a, 0.1, -9.8, 0.016);
    step(b, -0.1, -9.8, 0.016);
    step(pinned, 1, 1, 1);
  }
  console.log(a, b, pinned);
  console.log(collide(a, b), collide(a, far), collide(b, pinned), a, b);
  console.log(momentum(a, b), momentum(a, a), momentum(pinned, far));

  // The same body twice: separate copies would lose one of the updates, so the glue calls
  // the JS version.
  const self = body(1, 1, 2, 3);
  console.log(collide(self, self), self);

  // Extra properties survive; copy-back only touches the struct's fields.
  const tagged = { ...body(0, 0, 0, 0), name: "tagged" };
  step(tagged, 1, 2, 0.5);
  console.log(tagged);
}
