import { lerp } from "./helpers.js";
import { area, curve, describe } from "./kernel.js";

export function main(): void {
  const out = new Float64Array(10);
  curve(0, 1, out);
  console.log(Array.from(out).join(" "));
  console.log(area(0, 1), area(-5, 5), area(NaN, 1), describe(1, 2), lerp(1, 3, 0.25));
}
