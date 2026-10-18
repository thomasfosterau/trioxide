// A named function expression calls itself through its own name: `self(3)` is 6, not 1.
const fact = function self(n: number): number {
  return n <= 1 ? 1 : n * self(n - 1);
};

export function main(): void {
  console.log(fact(3));
}
