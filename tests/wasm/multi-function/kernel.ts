import { lerp, smooth } from "./helpers.js";

const STEPS = 8;

function sample(a: number, b: number, i: number): number {
  return lerp(a, b, smooth(i / STEPS));
}

/** Exported island functions calling each other and internal helpers. */
export function curve(a: number, b: number, out: Float64Array): void {
  "use wasm";
  for (let i = 0; i < out.length; i++) out[i] = sample(a, b, i);
}

export function area(a: number, b: number): number {
  "use wasm";
  const out = new Float64Array(STEPS + 1);
  curve(a, b, out);
  let total = 0;
  for (let i = 0; i < out.length; i++) total += out[i];
  return total / out.length;
}

/** Not forced: stays JS, but may still call the island's JS functions. */
export function describe(a: number, b: number): string {
  return `${a}..${b}: ${area(a, b).toFixed(6)}`;
}
