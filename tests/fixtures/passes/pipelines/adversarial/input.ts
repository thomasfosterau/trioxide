// Adversarial: chains that must stay calls (or keep their exact semantics).

function thisArg(): number[] {
  const scale = { by: 3 };
  // A function callback reads `this`, which the `thisArg` sets.
  return [1, 2].map(function (this: { by: number }, x) {
    return x * this.by;
  }, scale);
}

function selfReference(): number[] {
  // A named function expression that refers to itself (on a path these elements don't take).
  return [3, 4].map(function again(n: number): number {
    return n > 100 ? again(0) : n * 2;
  });
}

function usesArguments(): number[] {
  return [5, 6].map(function () {
    // eslint-disable-next-line prefer-rest-params
    return arguments.length;
  });
}

function intermediate(): number[] {
  // The second callback receives the intermediate array.
  return [1, 2, 3].map((x) => x * 2).map((x, i, all) => x + all.length);
}

function afterFilter(): number[] {
  // The index after a `filter` is the filtered array's.
  return [5, 6, 7].filter((x) => x !== 6).map((x, i) => x * 10 + i);
}

function startIndex(): boolean {
  return [1, 2, 3].includes(1, 1);
}

function zeros(): boolean[] {
  const xs = [-0, NaN];
  // SameValueZero: -0 equals 0, and NaN equals NaN.
  const zero = xs.includes(0);
  const nan = xs.includes(NaN);
  return [zero, nan, xs.indexOf(NaN) === -1];
}

function tdz(): string {
  try {
    // The callback reads the binding the chain initialises, in its temporal dead zone.
    const t: number[] = [1].map(() => t.length).map((v) => v);
    return String(t);
  } catch (error) {
    return error instanceof ReferenceError ? "ReferenceError" : "other";
  }
}

function asyncCallback(): number {
  const promises = [1, 2].map(async (x) => x);
  return promises.length;
}

function earlyReturn(): number[] {
  return [1, 2, 3].map((x) => {
    if (x === 2) {
      return 0;
    }
    return x;
  });
}

export function main(): void {
  console.log(thisArg());
  console.log(selfReference(), usesArguments(), intermediate(), afterFilter());
  console.log(startIndex(), zeros(), tdz(), asyncCallback(), earlyReturn());
}
