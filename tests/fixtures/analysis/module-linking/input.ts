// The linker resolves imports through re-exports, star exports, defaults and namespaces.

import area, { PI_ISH as pi, double } from "./reexports.js";
import * as geometry from "./geometry.js";
import { counter } from "./geometry.js";

export function main(): void {
  console.log(pi /*@proven number*/, double(21) /*@proven number int*/);
  console.log(area(2) /*@proven number*/);
  console.log(geometry.perimeter(3) /*@proven number int*/);
  console.log(counter.count /*@proven number int*/);
}
