// Generates the golden tables that `tests/golden.rs` checks trioxide_rt against: what V8
// computes for every runtime helper, on edge cases (NaN, ±0, ±Infinity, subnormals, 2^31 and
// 2^32 boundaries, rounding ties) plus deterministic pseudo-random values.
//
//   node crates/trioxide_rt/tests/golden/generate.mjs
//
// Each line is `name arg… result`. Numbers are printed so that parsing them back gives the
// exact double: shortest round-trip decimal, with `-0`, `NaN`, `Infinity` spelled out.

import { writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

const fmt = (x) => (Object.is(x, -0) ? '-0' : String(x));

// mulberry32: a small deterministic PRNG.
let seed = 0x7472696f; // "trio"
function random() {
  seed = (seed + 0x6d2b79f5) | 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const randomInt32 = () => (random() * 4294967296) | 0;
const bits = new DataView(new ArrayBuffer(8));
function randomBits() {
  bits.setUint32(0, (random() * 4294967296) >>> 0);
  bits.setUint32(4, (random() * 4294967296) >>> 0);
  const x = bits.getFloat64(0);
  return Number.isNaN(x) ? 1.25 : x;
}
function randomDoubles(n) {
  const out = [];
  for (let i = 0; i < n; i++) {
    switch (i % 5) {
      case 0: out.push((random() - 0.5) * 20); break;
      case 1: out.push(randomBits()); break;
      case 2: out.push(randomInt32() + (random() < 0.5 ? 0.5 : random())); break;
      case 3: out.push((random() - 0.5) * 2e10); break;
      default: out.push(Math.round((random() - 0.5) * 600) / 2); break;
    }
  }
  return out;
}

const special = [
  NaN, 0, -0, Infinity, -Infinity,
  Number.MIN_VALUE, -Number.MIN_VALUE, 2.2250738585072009e-308, -2.2250738585072009e-308,
  2.2250738585072014e-308, -2.2250738585072014e-308, Number.MAX_VALUE, -Number.MAX_VALUE,
  Number.EPSILON, 1, -1, 0.5, -0.5, 1.5, -1.5, 2.5, -2.5, 3.5, -3.5,
  0.49999999999999994, -0.49999999999999994, 0.5000000000000001, -0.5000000000000001,
  0.1, -0.1, 0.7, -0.7, 0.9999999999999999, 1 / 3, 3, 7, -7, 100, 123.456, -123.456,
  2 ** 31 - 1, 2 ** 31, -(2 ** 31), -(2 ** 31) - 1, 2 ** 31 - 0.5, -(2 ** 31) + 0.5, 2 ** 31 + 0.5,
  2 ** 32 - 1, 2 ** 32, 2 ** 32 + 1, -(2 ** 32), 4294967295.5, 2 ** 33 + 5,
  2 ** 52 - 0.5, 2 ** 52, 2 ** 52 + 1, 2 ** 53, 2 ** 53 + 2, -(2 ** 53), 2 ** 64, 1e21, -1e21, 1e308,
  127.5, 128, -128.5, 254.5, 255, 255.5, 256, -255.5, 32767.5, 65535.5, 65536,
  Math.PI, -Math.PI, Math.PI / 2, Math.E, Math.LN2, Math.SQRT2, 1e-7, 1e-300, -1e-300, 709.78, -745.2,
];
// A smaller set for the cartesian products of binary and ternary functions.
const pairSet = [
  NaN, 0, -0, Infinity, -Infinity, Number.MIN_VALUE, -Number.MIN_VALUE, 1, -1, 0.5, -0.5, 2.5,
  -2.5, 3, -7, 31, 32, 33, 1e-300, 2 ** 31 - 1, 2 ** 31, -(2 ** 31), 2 ** 32 + 1, 2 ** 53,
  Number.MAX_VALUE, -123.456, Math.PI,
];
const tripleSet = [NaN, 0, -0, Infinity, -Infinity, Number.MIN_VALUE, 1, -1, 0.5, 3, 1e300, 2 ** 53, -123.456];
const int32s = [
  0, 1, -1, 2, -2, 3, -3, 7, -7, 31, 32, 33, -32, 1000, -1000, 65535, 0x55555555,
  2 ** 31 - 1, -(2 ** 31), -(2 ** 31) + 1, 2 ** 30,
];

const unaryValues = [...special, ...randomDoubles(250)];
const pairs = [];
for (const a of pairSet) for (const b of pairSet) pairs.push([a, b]);
for (let i = 0; i < 300; i++) pairs.push(randomDoubles(2));
const triples = [];
for (const a of tripleSet) for (const b of tripleSet) for (const c of tripleSet) triples.push([a, b, c]);
const intPairs = [];
const ints = [...int32s, ...Array.from({ length: 12 }, randomInt32)];
for (const a of ints) for (const b of ints) intPairs.push([a, b]);
// Index arguments for the checked array read, over a 4-element array.
const indices = [...special, 3, 4, -0, 2, 1.0000000000000002, 2 ** 32 + 1];

const typed = (Ctor) => (x) => {
  const array = new Ctor(1);
  array[0] = x;
  return array[0];
};
const float64 = new Float64Array([10.5, -20, 30, 40]);

const unary = {
  round: Math.round, sign: Math.sign, trunc: Math.trunc, floor: Math.floor, ceil: Math.ceil,
  abs: Math.abs, sqrt: Math.sqrt, fround: Math.fround, clz32: Math.clz32,
  to_int32: (x) => x | 0, to_uint32: (x) => x >>> 0, bit_not: (x) => ~x,
  to_boolean: (x) => (x ? 1 : 0), neg: (x) => -x,
  store_i8: typed(Int8Array), store_u8: typed(Uint8Array), store_u8_clamped: typed(Uint8ClampedArray),
  store_i16: typed(Int16Array), store_u16: typed(Uint16Array), store_i32: typed(Int32Array),
  store_u32: typed(Uint32Array), store_f32: typed(Float32Array), store_f64: typed(Float64Array),
};
const binary = {
  max: Math.max, min: Math.min, imul: Math.imul, hypot2: Math.hypot,
  add: (a, b) => a + b, sub: (a, b) => a - b, mul: (a, b) => a * b, div: (a, b) => a / b, rem: (a, b) => a % b,
  bit_and: (a, b) => a & b, bit_or: (a, b) => a | b, bit_xor: (a, b) => a ^ b,
  shl: (a, b) => a << b, shr: (a, b) => a >> b, ushr: (a, b) => a >>> b,
  lt: (a, b) => +(a < b), le: (a, b) => +(a <= b), gt: (a, b) => +(a > b), ge: (a, b) => +(a >= b),
  eq: (a, b) => +(a === b), ne: (a, b) => +(a !== b), not_lt: (a, b) => +!(a < b),
};
const ternary = {
  hypot3: Math.hypot, max3: Math.max, min3: Math.min,
};
const integer = {
  rem_i32: (a, b) => a % b, div_trunc_i32: (a, b) => (a / b) | 0, imul_i32: Math.imul,
  shl_i32: (a, b) => a << b, shr_i32: (a, b) => a >> b, ushr_i32: (a, b) => a >>> b,
};
const index = {
  // `+` is how arithmetic sees `undefined`: NaN.
  get: (i) => +float64[i],
  get_number_array: (i) => +[10.5, -20, 30, 40][i],
};
const transcendentalUnary = {
  sin: Math.sin, cos: Math.cos, tan: Math.tan, asin: Math.asin, acos: Math.acos, atan: Math.atan,
  sinh: Math.sinh, cosh: Math.cosh, tanh: Math.tanh, asinh: Math.asinh, acosh: Math.acosh,
  atanh: Math.atanh, exp: Math.exp, expm1: Math.expm1, log: Math.log, log1p: Math.log1p,
  log2: Math.log2, log10: Math.log10, cbrt: Math.cbrt,
};
const transcendentalBinary = { atan2: Math.atan2, pow: Math.pow };

function table(functions, argLists) {
  const lines = [];
  for (const [name, f] of Object.entries(functions)) {
    for (const args of argLists) lines.push([name, ...args, f(...args)].map((x) => (typeof x === 'string' ? x : fmt(x))).join(' '));
  }
  return lines;
}

const header = (what) =>
  `# Golden table: ${what}. Generated by generate.mjs on Node ${process.version}; do not edit.\n`;

const files = {
  'unary.txt': ['unary helpers', table(unary, unaryValues.map((x) => [x]))],
  'binary.txt': ['binary helpers and operators', table(binary, pairs)],
  'ternary.txt': ['three-argument Math functions', table(ternary, triples)],
  'int32.txt': ['int32 operators', table(integer, intPairs)],
  'index.txt': ['checked array reads of [10.5, -20, 30, 40]', table(index, indices.map((i) => [i]))],
  'transcendental.txt': [
    'transcendental Math functions',
    [...table(transcendentalUnary, unaryValues.map((x) => [x])), ...table(transcendentalBinary, pairs)],
  ],
};
for (const [file, [what, lines]] of Object.entries(files)) {
  writeFileSync(path.join(here, file), header(what) + lines.join('\n') + '\n');
}
