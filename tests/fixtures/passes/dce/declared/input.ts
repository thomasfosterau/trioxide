// Negative cases: in the library world `scale` may be called with anything, so whether a call
// to it is pure rests on declared types (`x * 2` could run valueOf on an object), and its
// result, though unused, stays.

export function scale(x: number): number {
  return x * 2;
}

export function main(): void {
  const unused = scale(3);
  scale(4);
  console.log("done");
}
