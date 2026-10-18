// Earlier top-level code patches `Math.floor`: the interpreter's model of it doesn't apply.
function floored(): number {
  return Math.floor(2.5);
}

(Math as { floor: (x: number) => number }).floor = (): number => 7;
const R = floored();

export function main(): void {
  console.log(R);
}
