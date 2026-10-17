import { fillC8, fillF32, fillF64, fillI16, fillI32, fillI8, fillU16, fillU32, fillU8, makeClamped, makeFloat32, total } from "./kernel.js";

const dump = (xs: ArrayLike<number>): string => Array.from(xs, (x) => (Object.is(x, -0) ? "-0" : String(x))).join(",");

export function main(): void {
  for (const v of [0.5, 1.5, 2.5, -1.5, 100.7, 300, -300, 70000.25, 2 ** 31 + 0.5, 1e20, NaN, -Infinity, -0]) {
    const i8 = new Int8Array(5), u8 = new Uint8Array(5), c8 = new Uint8ClampedArray(5);
    const i16 = new Int16Array(5), u16 = new Uint16Array(5), i32 = new Int32Array(5);
    const u32 = new Uint32Array(5), f32 = new Float32Array(5), f64 = new Float64Array(5);
    fillI8(i8, v); fillU8(u8, v); fillC8(c8, v); fillI16(i16, v); fillU16(u16, v);
    fillI32(i32, v); fillU32(u32, v); fillF32(f32, v); fillF64(f64, v);
    console.log(v, [i8, u8, c8, i16, u16, i32, u32, f32, f64].map(dump).join(" | "));
    console.log(total(i8, u16, f32));
  }
  const clamped = makeClamped(5);
  console.log(clamped.constructor.name, dump(clamped), dump(makeFloat32(4)), makeFloat32(0).length);
}
