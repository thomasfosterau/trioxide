// The negative cases with trustTypes: the calls to `scale` count as pure, so the unused
// result and the call statement go.

export function scale(x: number): number {
  return x * 2;
}

export function main(): void {
  const unused = scale(3);
  scale(4);
  console.log("done");
}
