// The module imports, so an import cycle could call its functions before a module-level
// `const` exists: callbacks are hoisted just before the outermost loop around them instead.
import { VALUES } from "./values.js";

function labeled(rounds: number): number {
  let total = 0;
  outer: for (let r = 0; r < rounds; r++) {
    for (let i = 0; i < 2; i++) {
      total += [r, i].map((v) => v + 1).length;
      if (total > 100) break outer;
    }
  }
  return total;
}

function guarded(go: boolean): number {
  let total = 0;
  if (go)
    for (let i = 0; i < 3; i++) {
      total += VALUES.map((v) => v * 2)[i];
    }
  return total;
}

function once(): number[] {
  // Not in a loop: it would need a module-level constant.
  return VALUES.map((v) => v - 1);
}

export function main(): void {
  console.log(labeled(3), guarded(true), guarded(false), once().join(","));
}
