import { collatz } from "./kernel.js";

export const run = (): number => {
  return collatz(30000);
};

export function main(): void {
  console.log(run());
}
