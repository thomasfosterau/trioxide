// Call-site completeness, expression and loop effects, points-to and reachability.

const external = JSON.parse('{}') as { unknown?: (n: number) => number };

let log = 0;

function bump(n: number): number {
  log += n;
  return n;
}

function square(x: number): number {
  return x * x;
}

function never(): number {
  return 1;
}

function early(flag: boolean): number {
  if (flag) {
    return 1;
  }
  return 2;
  log = 99;
}

function loop(values: number[]): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    total += values[i] * values[i];
  }
  return total;
}

function popping(values: number[]): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    values.pop();
    total += i;
  }
  return total;
}

function invariant(values: number[], k: number): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    total += values[i] + k * 3;
  }
  return total;
}

function guarded(values: number[], o: { x: number } | null): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    if (o !== null) {
      total += o.x * 2;
    }
  }
  return total;
}

function clock(values: number[]): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    total += Date.now() * 0;
  }
  return total;
}

function boxed(values: number[]): number {
  const box = { v: 1 };
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    total += box.v * 2;
    box.v++;
  }
  return total;
}

function captured(values: number[]): number {
  let c = 1;
  const inc = (): void => {
    c++;
  };
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    total += c * 2;
    inc();
  }
  return total;
}

class Scale {
  factor = 2;
  apply(x: number): number {
    return x * this.factor;
  }
}

function method(values: number[], scale: Scale): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    total += scale.apply(i);
  }
  return total;
}

function exits(values: number[], k: number): number {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    if (i > 3) {
      break;
    }
    total += k * 5;
  }
  return total;
}

function locals(): number {
  const arr = [1, 2, 3];
  let t = 0;
  for (let i = 0; i < 3; i++) {
    t += arr[0] * 2;
    arr[0] = i + 10;
    arr.push(i);
  }
  return t;
}

function getter(): number {
  let k = 0;
  const getK = (): number => k;
  let t = 0;
  for (let i = 0; i < 3; i++) {
    t += getK() * 1;
    k++;
  }
  return t;
}

// Units the engine calls: their sites must not read as complete.
async function thenable(): Promise<number> {
  const t = {
    then(r: (v: number) => void): void {
      log += 3;
      r(1);
    },
  };
  return await t;
}

function coerced(): string {
  const o = {
    toString(this: { k(): void }): string {
      this.k();
      return "o";
    },
    k(): void {
      log += 1;
    },
  };
  return `${o}`;
}

function promised(): Promise<number> {
  return new Promise((resolve) => {
    log += 1;
    resolve(1);
  });
}

export function main(): void {
  const point = { x: 1 };
  const pure = square(3);
  const effect = bump(2);
  const outside = external.unknown?.(1);
  console.log(point.x, pure, effect, outside, early(true), early(false));
  console.log(loop([1, 2, 3]), popping([1, 2, 3]), invariant([1, 2], 4), invariant([], 5));
  console.log(guarded([1, 2], { x: 3 }), guarded([1], null), clock([1, 2]), boxed([1, 2, 3]));
  void thenable();
  void promised();
  console.log(coerced(), log, locals(), getter());
  console.log(captured([1, 2, 3]), method([1, 2], new Scale()), exits([1, 2, 3, 4, 5, 6], 2));
}
