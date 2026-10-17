// Adversarial cases for DCE: nothing that may throw or run user code goes.

const log: string[] = [];

const tracked = {
  get noisy(): number {
    log.push("getter");
    return 1;
  },
};

const primitive = {
  [Symbol.toPrimitive](hint: string): number {
    log.push(`toPrimitive ${hint}`);
    return 3;
  },
};

function tdz(): string {
  try {
    // Reads `later` before its declaration: throws, so it stays.
    const unused = later;
    const later = 1;
    return "no error";
  } catch (error) {
    return (error as Error).name;
  }
}

function hoisted(): number {
  return early();
  // Unreachable, but hoisted: called above.
  function early(): number {
    return 7;
  }
}

function deadVar(): unknown {
  const before = typeof v;
  return before;
  var v = 1;
}

function iterates(): number {
  const items = new Set([1, 2]);
  // Iterating runs the iterator protocol.
  for (const _ of items) {
  }
  const [first] = items;
  return first;
}

export const entryExport = 5;

export function main(): void {
  const viaGetter = tracked.noisy;
  const viaPrimitive = +(primitive as unknown as number);
  const viaTemplate = `${primitive as unknown as string}`;
  console.log(tdz(), hoisted(), deadVar(), iterates(), log);
}
