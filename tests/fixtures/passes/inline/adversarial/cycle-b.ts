import { Cyclic } from "./cycle-a.js";

export function readB(): number {
  // In an import cycle: the enum object might not exist yet when this runs.
  return Cyclic.X;
}
