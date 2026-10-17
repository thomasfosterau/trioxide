"use wasm";
// Direct and mutual recursion compile to plain Rust recursion.

export function fib(n: number): number {
  return n < 2 ? n : fib(n - 1) + fib(n - 2);
}

export function gcd(a: number, b: number): number {
  return b === 0 ? a : gcd(b, a % b);
}

export function isEven(n: number): boolean {
  return n === 0 ? true : isOdd(n - 1);
}

function isOdd(n: number): boolean {
  return n === 0 ? false : isEven(n - 1);
}

export function power(base: number, exponent: number): number {
  if (exponent <= 0) return 1;
  const half = power(base, Math.floor(exponent / 2));
  return exponent % 2 === 0 ? half * half : half * half * base;
}
