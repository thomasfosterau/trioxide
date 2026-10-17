import { fib, gcd, isEven, power } from "./kernel.js";

export function main(): void {
  console.log([0, 1, 2, 10, 20, 25].map(fib).join(" "));
  console.log(gcd(48, 18), gcd(17, 5), gcd(0, 9), gcd(2.5, 1), gcd(-12, 8));
  console.log([0, 1, 7, 100].map(isEven).join(" "));
  console.log(power(2, 10), power(1.5, 7), power(-2, 31), power(10, 0), power(3, 2.5));
}
