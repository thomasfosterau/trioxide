import { brightCells } from "./kernel.js";

export const run = (): number => {
  return brightCells(7, 640, 480, 200) + brightCells(42, 640, 480, 250);
};

export function main(): void {
  console.log(run());
}
