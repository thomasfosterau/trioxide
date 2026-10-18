import { run, tagged } from "./kernel.js";
// Runs at module evaluation, before `ready()`.
const early = run(3);
export function main(): void {
  console.log(early, run(5), tagged(3, 4));
}
