import { rounds } from "./kernel.js";

export const run = (): number => {
  return rounds(1000000);
};

export function main(): void {
  console.log(run());
}
