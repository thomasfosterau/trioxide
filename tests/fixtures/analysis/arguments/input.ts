// `arguments`: in strict code (modules) it doesn't alias parameters, but it does expose the
// call's actual arguments.

function first(a: number): unknown {
  return arguments[0];
}

function count(...rest: number[]): number {
  return arguments.length + rest.length;
}

function viaArguments(x: number): number {
  const args = arguments;
  args[0] = 100;
  // Strict mode: x is unaffected.
  return x /*@proven number int*/;
}

export function main(): void {
  console.log(first(1) /*@unknown*/, count(1, 2) /*@declared number*/, viaArguments(5));
}
