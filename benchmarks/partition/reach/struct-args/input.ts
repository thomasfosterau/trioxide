import { distance } from "./kernel.js";

export function main(): void {
  let total = 0;
  for (let i = 0; i < 100; i++) total += distance({ x: i, y: 0 }, { x: 0, y: i });
  console.log(total);
}
