// A program that patches a builtin loses that builtin's model everywhere.

(Math as { floor: (x: number) => unknown }).floor = () => "patched";

export function main(): void {
  console.log(Math.floor(1.5) /*@declared number*/, Math.ceil(1.5) /*@declared number*/);
  console.log(Number.isInteger(1) /*@proven boolean*/);
}
