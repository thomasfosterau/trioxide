// A function whose only calls partial evaluation replaced goes; one still referenced stays.

// A table built when the module loads.

function squares(n: number): number[] {
  const out: number[] = [];
  for (let i = 0; i < n; i++) out.push(i * i);
  return out;
}

const SQUARES = squares(6);

function factorial(n: number): number {
  return n <= 1 ? 1 : n * factorial(n - 1);
}

const F5 = factorial(5);
const F6 = factorial(6);

function cube(n: number): number {
  return n * n * n;
}

const C3 = cube(3);

export function doubled(n: number): number {
  return n * 2;
}

const D4 = doubled(4);

function half(n: number): number {
  return n / 2;
}

const H8 = half(8);
const halve = half;

function tripled(n: number): number {
  return n * 3;
}

const T2 = tripled(2);
const T = tripled(SQUARES.length);

export function main(): void {
  console.log(SQUARES.join(), F5, F6, C3, cube(SQUARES.length), D4, H8, halve(3), T2, T);
}
