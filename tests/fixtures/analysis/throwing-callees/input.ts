// Throws through callers, arrows, constructors, field initialisers and static blocks.

let log = 0;

function thrower(n: number): number {
  if (n > 2) {
    throw "big";
  }
  return n;
}

const viaArrow = (n: number): number => thrower(n);

const viaBlockArrow = (n: number): number => {
  return thrower(n) + 1;
};

function viaFunction(n: number): number {
  return viaArrow(n) * 2;
}

const writesArrow = (n: number): number => (log += n);

class Checked {
  value: number;
  constructor(n: number) {
    this.value = thrower(n);
  }
}

class Initialised {
  first = thrower(1);
  second = writesArrow(1);
}

class Static {
  static ready = 0;
  static {
    Static.ready = thrower(2);
    log += 1;
  }
}

function guarded(n: number): number {
  try {
    return viaFunction(n);
  } catch {
    return -1;
  }
}

export function main(): void {
  const results = [guarded(1), guarded(5), viaBlockArrow(1), writesArrow(2)];
  let caught = 0;
  try {
    viaBlockArrow(9);
  } catch {
    caught += 1;
  }
  try {
    new Checked(7);
  } catch {
    caught += 1;
  }
  console.log(results, caught, new Checked(1).value, new Initialised().second, Static.ready, log);
}
