// `fold` must not replace `fact(3)` with 1: the recursive branch runs (`self` is the function).
const fact = function self(n: number): number {
  return n <= 1 ? 1 : n * self(n - 1);
};

export function main(): void {
  console.log(fact(3));
}
