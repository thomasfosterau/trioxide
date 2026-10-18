import { early } from "./cycle.js";

const N = 5;

export function getN(i: number): number {
  return N * i;
}

export { early };
