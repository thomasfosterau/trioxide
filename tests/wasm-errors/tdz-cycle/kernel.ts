// A module in an import cycle may be read before its body runs: `cycle.ts` calls `getN`
// before `N` is initialised, which throws in JS.
"use wasm";
import { early } from "./cycle.js";
const N = 5;
export function getN(i: number): number { return N * i; }
export { early };
