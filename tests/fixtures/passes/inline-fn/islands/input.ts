import { hot, run, table } from "./kernel.js";
// Runs at module evaluation, before `ready()`: the island calls stay calls.
const early = run(3);
const folded = table(4);
const direct = hot(2);
export function main(): void {
  console.log(early, folded, direct, run(5), table(3), hot(4));
}
