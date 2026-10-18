// Positive cases: unreachable code, unused pure bindings and functions, unused exports and
// pure expression statements go.
import { used } from "./util.js";

function neverCalled(): number {
  return 1;
}

function early(flag: boolean): string {
  if (flag) {
    return "early";
  }
  return "late";
  console.log("unreachable");
}

function loops(): number {
  let total = 0;
  for (let i = 0; i < 3; i++) {
    total += i;
    continue;
    total += 100;
  }
  return total;
}

export function main(): void {
  const unusedNumber = 1 + 2;
  const unusedObject = { a: 1, b: [2, 3] };
  const unusedCall = Math.max(1, 2);
  const kept = used(1);
  kept * 2;
  "no effect";
  console.log(kept, early(true), early(false), loops());
}
