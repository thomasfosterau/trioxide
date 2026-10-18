// A named function or class expression's own name, inside its body, is the function (class)
// itself: calls through it are calls of that function, with their arguments.
export function main(): void {
  const factorials = [3, 4].map(function fact(n: number): number {
    return n <= 1 ? 1 : n * fact(n - 1);
  });
  console.log(factorials.join(","));

  const C = class K {
    value: number;
    constructor(value: number) {
      this.value = value;
    }
    static make(value: number): K {
      return new K(value * 2);
    }
  };
  console.log(C.make(5).value);
}
