import { bounds, move, type Particle, speed, sum } from "./kernel.js";

function particle(x: number, y: number, vx: number, vy: number): Particle {
  return { x, y, vx, vy, alive: true };
}

export function main(): void {
  const ps = [particle(0, 0, 1, 2), particle(9, 0, 3, -1), particle(-0, 5, -0.5, 0.25)];
  for (let step = 0; step < 4; step++) for (const p of ps) move(p, 0.5);
  console.log(ps, ps.map(speed));
  const b = bounds(new Float64Array([3, -1, 4.5, -0, 9]));
  console.log(b, Object.keys(b), bounds(new Float64Array(0)));
  console.log(sum(new Float64Array([1, 2, 3]), new Float64Array([4, 5, 6])), sum(new Float64Array([1, 2]), new Float64Array([1])));
}
