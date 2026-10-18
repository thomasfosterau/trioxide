import { scale } from "./kernel.js";

export function main(): void {
  for (const own of [6, 2]) {
    const a = new Float64Array([1, 2, 3, 4]);
    Object.defineProperty(a, "length", { value: own });
    console.log(scale(a, 2), Array.from(new Float64Array(a.buffer)).join(","));
  }

  const buffer = new ArrayBuffer(16, { maxByteLength: 64 });
  const tracking = new Float64Array(buffer);
  tracking.set([1, 2]);
  console.log(scale(tracking, 3), Array.from(tracking).join(","));
  buffer.resize(32);
  tracking[3] = 5;
  console.log(scale(tracking, 3), Array.from(tracking).join(","));
  const fixed = new Float64Array(buffer, 8, 2);
  console.log(scale(fixed, 10), Array.from(tracking).join(","));
  buffer.resize(16);
  console.log(fixed.length, scale(fixed, 10), Array.from(tracking).join(","));
}
