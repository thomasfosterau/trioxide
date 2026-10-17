// Methods the engine calls through a protocol (`Symbol.toPrimitive`, iterators, `toJSON`,
// `Symbol.hasInstance`) have callers trioxide doesn't see: they are open.

let log = "";

const primitive = {
  [Symbol.toPrimitive](hint: string): number {
    log += hint;
    return hint === "number" ? 1 : 2;
  },
};

const iterable = {
  [Symbol.iterator](): Iterator<number> {
    let n = 0;
    return {
      next(): IteratorResult<number> {
        n += 1;
        log += "n";
        return n > 2 ? { done: true, value: 0 } : { done: false, value: n };
      },
    };
  },
};

class Range {
  constructor(readonly end: number) {}

  *[Symbol.iterator](): Generator<number> {
    for (let i = 0; i < this.end; i++) {
      log += "r";
      yield i;
    }
  }
}

class Even {
  static [Symbol.hasInstance](value: unknown): boolean {
    log += "h";
    return typeof value === "number" && value % 2 === 0;
  }
}

const serialised = {
  value: 3,
  toJSON(key: string): string {
    log += "j" + key;
    return "three";
  },
};

function sum(values: Iterable<number>): number {
  let total = 0;
  for (const value of values) {
    total += value;
  }
  return total;
}

export function main(): void {
  console.log(+(primitive as any), `${primitive as any}`);
  console.log(sum(iterable), sum(new Range(3)), [...iterable].length);
  console.log((4 as any) instanceof Even, JSON.stringify({ s: serialised }));
  console.log(log);
}
