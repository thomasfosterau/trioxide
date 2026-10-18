import { hashRounds } from "./kernel.js";

export const run = (): number => {
  return hashRounds(12345, 2_000_000);
};

export function main(): void {
  console.log(run());
}
