import { sdf } from "./kernel.js";

export const run = (): number => {
  return sdf(400, 300);
};

export function main(): void {
  console.log(run());
}
