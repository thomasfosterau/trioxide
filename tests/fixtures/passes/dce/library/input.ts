// Library world: exports stay even when nothing in the program imports them.
import { helper } from "./util.js";

export function unusedExport(): number {
  return 3;
}

export function main(): void {
  console.log(helper());
}
