// Interval, integer, NaN and -0 facts through arithmetic and Math.

/** Uses a value without printing it (it depends on `Math.random()`). */
function sink(..._values: unknown[]): void {}

export function main(): void {
  const r = Math.random();
  const byte = (r * 256) | 0;
  sink(byte /*@proven number int nonNaN*/);
  const masked = (r * 1e9) & 0xff;
  sink(masked /*@proven number int*/);
  const unsigned = -1 >>> 0;
  console.log(unsigned /*@proven number int nonNaN*/);
  const half = byte / 2;
  sink(half /*@proven number nonNaN*/, Math.floor(half) /*@proven number int*/);
  const negativeZero = -0 * 5;
  console.log(Object.is(negativeZero, -0), negativeZero /*@proven number*/);
  const nan = 0 / 0;
  console.log(Number.isNaN(nan), nan /*@proven number*/);
  const rounded = Math.round(-0.4);
  console.log(Object.is(rounded, -0), rounded /*@proven number int*/);
  const clamped = Math.min(255, Math.max(0, byte * 2));
  sink(clamped /*@proven number int nonNaN*/);
  const big = 2 ** 53 + 1;
  console.log(big /*@proven number*/, Number.MAX_SAFE_INTEGER /*@proven number int*/);
  let i = 0;
  while (i < 10) {
    i += 3;
  }
  console.log(i /*@proven number int*/);
  const remainder = byte % 7;
  sink(remainder /*@proven number int*/);
  const code = "abc".charCodeAt(5);
  console.log(code /*@proven number*/, Number.isNaN(code));
}
