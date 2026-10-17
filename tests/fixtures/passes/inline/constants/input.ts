// Positive cases: proven constants inline as literals, across modules too.
import { BANNER, Color, ENABLED, HUGE, LIMIT, NAME, NOTHING, OFFSET, ONE_BIG } from "./config.js";

const enum Mode {
  Fast = "fast",
  Slow = "slow",
}

enum Level {
  Low,
  High = 10,
}

const NEGATIVE_ZERO = -0;
const NOT_DEFINED = undefined;

function describe(level: Level): string {
  return level === Level.High ? "high" : "low";
}

export function main(): void {
  console.log(LIMIT * 2, NAME, ENABLED, NOTHING, ONE_BIG, OFFSET);
  // Not a literal, so not inlined (but folded where the type is a single value).
  console.log(HUGE);
  // Too long to copy.
  console.log(BANNER.length);
  console.log(Color.Blue, Color.Red, Mode.Fast, Mode["Slow"], describe(Level.High));
  console.log(Object.is(NEGATIVE_ZERO, -0), 1 / NEGATIVE_ZERO, NOT_DEFINED);
}
