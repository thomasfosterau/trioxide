// Accessors in object literals, `__proto__` in literals and spread sources run user code: their
// effects reach the unit doing the write, read or spread.

let log = 0;

function viaSetter(): number {
  const s = {
    set p(n: number) {
      log += n;
    },
  };
  s.p = 3;
  return 0;
}

function viaGetter(): number {
  const g = {
    get p(): number {
      log += 1;
      return 1;
    },
  };
  return g.p;
}

function viaProto(): number {
  const o = {
    __proto__: {
      set p(n: number) {
        log += n;
      },
    },
  } as any;
  o.p = 1;
  return 0;
}

function inLoop(): number {
  const s = {
    set p(n: number) {
      log += n;
    },
  };
  for (let i = 0; i < 2; i++) {
    s.p = 1;
  }
  return 0;
}

function viaSpread(): number {
  const src = {
    get x(): number {
      log += 10;
      return 1;
    },
  };
  const c = { ...src };
  return c.x;
}

function spreadParam(s: { x: number }): number {
  const c = { ...s };
  return c.x;
}

function spreadOnly(s: { x: number }): number {
  const c = { ...s };
  return c === s ? 1 : 0;
}

export function main(): void {
  console.log(viaSetter(), viaGetter(), viaProto(), inLoop(), viaSpread(), log);
  const getter = {
    get x(): number {
      log += 100;
      return 2;
    },
  };
  console.log(spreadParam(getter), spreadOnly(getter), log);
}
