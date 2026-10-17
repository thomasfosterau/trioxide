// Recursion and mutual recursion: summaries over strongly connected components.

function fib(n: number): number {
  return n < 2 ? n : fib(n - 1) + fib(n - 2);
}

function isEven(n: number): boolean {
  return n === 0 ? true : isOdd(n - 1);
}

function isOdd(n: number): boolean {
  return n === 0 ? false : isEven(n - 1);
}

type List = { value: number; next: List | null };

function sum(list: List | null): number {
  return list === null ? 0 : list.value + sum(list.next);
}

export function main(): void {
  const list: List = { value: 1, next: { value: 2, next: null } };
  console.log(fib(10) /*@proven number*/, isEven(4) /*@proven boolean*/, sum(list) /*@proven number*/);
}
