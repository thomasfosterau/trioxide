// Object-heavy simulation: 2000 particles, immutable Vec2 temporaries, class methods and
// object literals in the hot loop.
import { centroid, energy, makeParticles, spread, step } from "./world.js";

export const run = (): number => {
  const particles = makeParticles(2000);
  for (let i = 0; i < 20; i++) step(particles, 0.01);
  const center = centroid(particles);
  return Math.round(energy(particles) * 1000 + center.x * 1e6 + center.y * 1e3 + spread(particles, center) * 1e4);
};

export function main(): void {
  console.log(run());
}
