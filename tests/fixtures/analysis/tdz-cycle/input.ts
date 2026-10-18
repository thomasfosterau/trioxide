// A module-level `const` read through an import cycle before its module has run: `kernel.ts`
// imports `cycle.ts`, which runs first and calls `getN` before `N` is initialised.
import { early, getN } from "./kernel.js";

export function main(): void {
  console.log(early, getN(3));
}
