// Adversarial cases for folding.

const counter = {
  reads: 0,
  get value(): number {
    this.reads++;
    return 1;
  },
};

const coercing = {
  valueOf(): number {
    console.log("valueOf ran");
    return 2;
  },
};

function getters(): number {
  // A getter runs on every read: the read stays.
  return counter.value + counter.value;
}

function primitive(): number {
  // `+` runs valueOf.
  return +(coercing as unknown as number) * 1;
}

function hoisting(): unknown {
  // The branch never runs, but its `var` is visible outside it.
  if (Math.random() > 2) {
    var hoisted = 1;
  }
  return hoisted;
}

function shadowedNames(NaN: number, Infinity: number, Math: { floor(x: number): number }): unknown[] {
  // NaN, Infinity and Math are parameters: folds must not lean on the globals.
  return [0 / 0, 1 / 0, -1 / 0, Math.floor(2.5), NaN, Infinity];
}

function negativeZero(): number[] {
  const z = -0;
  // max(-0, 0) is 0, min(0, -0) is -0.
  return [Math.max(z, 0), Math.min(0, z), Math.max(0, z), Math.abs(z)];
}

function sideEffects(): number {
  let n = 0;
  // The comparison is decided, but evaluating it writes n.
  if ((n = 5) > 10) {
    return -1;
  }
  return n;
}

export function main(): void {
  console.log(getters(), counter.reads, primitive(), hoisting());
  console.log(shadowedNames(1, 2, { floor: (x: number) => x * 10 }));
  console.log(negativeZero().map((x) => Object.is(x, -0)), sideEffects());
}
