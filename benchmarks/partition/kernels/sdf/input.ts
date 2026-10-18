import { sdf } from "./kernel.js";

export function run(): number {
  return sdf(400, 300);
}

export function main(): void {
  console.log(run());
}
