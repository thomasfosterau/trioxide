import { atans } from "./kernel.js";

export const run = (): number => {
  return atans(200000);
};

export function main(): void {
  console.log(run().toFixed(6));
}
