import {
  addAfterWrite, addAll, bumpC8, bumpF32, bumpF64, bumpI16, bumpI32, bumpI8, bumpU16, bumpU32, bumpU8,
  hash, histogram, readInts,
} from "./kernel.js";

const dump = (xs: ArrayLike<number>): string => Array.from(xs, (x) => (Object.is(x, -0) ? "-0" : String(x))).join(",");

export function main(): void {
  for (const i of [0, 1, 2, 3, -1, 0.5, -0, NaN, 2 ** 32]) {
    const i8 = Int8Array.of(127, -128, 0), u8 = Uint8Array.of(255, 0, 1), c8 = Uint8ClampedArray.of(255, 254, 0);
    const i16 = Int16Array.of(-32768, 0, 1), u16 = Uint16Array.of(0, 65535, 1);
    const i32 = Int32Array.of(2147483647, -2147483648, 0), u32 = Uint32Array.of(0, 2, 4294967295);
    const f32 = Float32Array.of(16777216, 0.5, -1), f64 = Float64Array.of(2 ** 53, -0, 0.25);
    bumpI8(i8, i); bumpU8(u8, i); bumpC8(c8, i); bumpI16(i16, i); bumpU16(u16, i);
    bumpI32(i32, i); bumpU32(u32, i); bumpF32(f32, i); bumpF64(f64, i);
    console.log(i, [i8, u8, c8, i16, u16, i32, u32, f32, f64].map(dump).join(" | "));
  }
  for (const k of [1, -1, 2 ** 31 - 1, -(2 ** 31), 2 ** 32 + 5, 0.5, NaN]) {
    const xs = Uint32Array.of(0, 1, 4294967295, 2147483648);
    addAll(xs, k);
    console.log(k, dump(xs));
  }
  const bytes = new Uint8Array(4096);
  for (let i = 0; i < bytes.length; i++) bytes[i] = (i * 37 + (i >> 3)) & 255;
  const bins = new Uint32Array(16);
  bins[3] = 4294967295;
  histogram(bytes, bins);
  console.log(dump(bins));
  const target = Int32Array.of(5, 6);
  addAfterWrite(target, 0);
  addAfterWrite(target, 9);
  console.log(dump(target));
  const xs = Float64Array.of(2 ** 31 + 0.5, -1.5, NaN, 1e20, -Infinity);
  const ys = Uint32Array.of(4294967295, 2147483648, 7, 0, 1);
  const zs = Int8Array.of(-128, 127, -1, 0, 3);
  for (const i of [0, 1, 2, 3, 4, 5, -1, 0.5]) console.log(i, readInts(xs, ys, zs, i));
  for (const seed of [0, 1, -1, 2 ** 31 - 1, 123456789]) console.log(seed, hash(seed, 3));
}
