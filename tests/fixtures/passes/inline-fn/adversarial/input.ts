// Traps for function inlining. Each must print the same as the original.
const order: string[] = [];
function next(tag: string): number {
  order.push(tag);
  return order.length;
}

// The callee reassigns its parameter: the argument is copied into a local, never aliased.
function bump(x: number): number {
  x = x + 1;
  return x * 2;
}

// Defaults evaluate after earlier parameters, in the callee's scope.
const x = 100;
function defaults(a: number, b: number = a + x): number {
  const x = 1;
  return a * b + x;
}

// A default parameter followed by a computed argument: the default would move.
function late(a: number = next("default"), b: number = 0): number {
  return a + b;
}

// Rest parameters.
function total(...values: number[]): number {
  let sum = 0;
  for (const value of values) sum += value;
  return sum;
}

// `return` inside a loop and inside `try`/`finally`.
function find(list: number[], wanted: number): number {
  for (let i = 0; i < list.length; i++) {
    if (list[i] === wanted) return i;
  }
  return -1;
}

function guarded(n: number): number {
  try {
    if (n > 1) return n * 10;
    order.push("small");
  } finally {
    order.push("finally");
  }
  return 0;
}

// Arguments run once and in order, even when the body uses them in another order.
function difference(a: number, b: number): number {
  return b - a;
}

// Extra arguments are still evaluated.
function first(a: number): number {
  return a;
}

// A name the callee reads means something else where it is called.
const scale = 3;
function scaled(v: number): number {
  return v * scale;
}

// What may not be inlined.
function self(this: { n: number } | void): number {
  return this ? (this as { n: number }).n : -1;
}
function count(): number {
  return arguments.length;
}
function* numbers(): Generator<number> {
  order.push("generator body");
  yield 1;
}
async function later(): Promise<number> {
  return 1;
}
function fact(n: number): number {
  return n <= 1 ? 1 : n * fact(n - 1);
}
function adder(n: number): () => number {
  return () => n + 1;
}

export function main(): void {
  const a = 5;
  console.log(bump(a), a);
  console.log(defaults(2), defaults(2, 3));
  console.log(late(undefined, next("argument")), order.join(","));
  const sum = total(1, 2, next("rest"));
  console.log(sum);
  const at = find([4, 5, 6], 6);
  const missing = find([4, 5, 6], 7);
  console.log(at, missing);
  const big = guarded(2);
  const small = guarded(0);
  console.log(big, small, order.join(","));
  console.log(difference(next("a"), next("b")), order.join(","));
  console.log(first(1, next("extra")), order.join(","));
  {
    const scale = 10;
    console.log(scaled(2), scale);
  }
  const gen = numbers();
  console.log(order.includes("generator body"), gen.next().value, order.includes("generator body"));
  console.log(self(), count(), fact(5), adder(1)(), typeof later());
  outer: for (let i = 0; i < 2; i++) {
    const found = find([i], i);
    if (found === 0) continue outer;
  }
}
