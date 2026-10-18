// `Math.abs(-0)` is `+0`; `-z` of `z = -0` is `+0`.
export function main(): void {
  const z = -0;
  console.log(Object.is(Math.abs(-0), 0), 1 / Math.abs(-0), 1 / Math.abs(z));
  console.log(Object.is(-z, 0), Object.is(-(-0), 0), Object.is(+(-0), -0));
}
