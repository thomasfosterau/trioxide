import { Particle, energy } from "./kernel.js";

export function main(): void {
  let total = 0;
  for (let i = 0; i < 100; i++) total += energy(new Particle(i, i / 2));
  console.log(total);
}
