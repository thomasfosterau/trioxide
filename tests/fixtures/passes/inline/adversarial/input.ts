// Adversarial cases for inlining.
import { SIDE } from "./side.js";
import { readA } from "./cycle-a.js";

// Enums first, so no code runs before them (the order is part of what makes their
// members constants).
enum Mutated {
  A = 1,
}
enum Escaping {
  A = 1,
}
// Changing an enum object: its members are no longer constants.
(Mutated as unknown as { A: number }).A = 7;
// Using one as a value: only TypeScript says its members are read-only.
const keys = Object.keys(Escaping);

function early(): string {
  // A read in the temporal dead zone throws; inlining it would hide the error.
  try {
    return String(LATE);
  } catch (error) {
    return (error as Error).name;
  }
}
const before = early();
const LATE = 1;

const holder = {
  get value(): number {
    console.log("getter ran");
    return 1;
  },
};
const viaGetter = holder.value;

let reassigned = 1;
reassigned = 2;

export function main(): void {
  console.log(before, LATE, SIDE);
  console.log(Mutated.A, Escaping.A, keys);
  console.log(viaGetter, reassigned, readA());
}
