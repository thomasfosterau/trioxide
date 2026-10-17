import { readB } from "./cycle-b.js";

export enum Cyclic {
  X = 5,
}

export function readA(): number {
  return readB();
}
