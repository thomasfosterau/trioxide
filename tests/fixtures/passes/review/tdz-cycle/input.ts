// A constant read from a function another module of an import cycle calls before the
// constant is initialised throws; inlining it would hide that.
import { describe } from "./e.js";
import { R3 } from "./x.js";
export function main(): void {
  console.log(describe(), R3);
}
