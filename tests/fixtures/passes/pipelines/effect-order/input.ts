// Adversarial: fusing runs callbacks element by element. That is only allowed when the order
// can't be observed.

const log: string[] = [];

function note(tag: string, value: number): number {
  log.push(`${tag}${value}`);
  return value;
}

function twoEffects(): number[] {
  const values = [1, 2, 3];
  // Both callbacks log: the chain stays, so the log reads f1 f2 f3 g2 g4 g6.
  return values.map((x) => note("f", x)).map((x) => note("g", x * 2));
}

function firstEffect(): number {
  const values = [1, 2, 3];
  // Only the first callback has effects and the second reads nothing: fused.
  return values
    .map((x) => note("h", x))
    .filter((x) => x !== 2)
    .reduce((s, x) => s + x, 0);
}

function lastEffect(): void {
  const values = [4, 5];
  // Only the last callback has effects, and the analysis can't rule out that they reach the
  // array (the builtin hands it to every callback): the chain stays.
  values.map((x) => x + 1).forEach((x) => {
    note("k", x);
  });
}

function observer(): number[] {
  const values = [1, 2, 3];
  let calls = 0;
  // The second callback reads what the first writes: the chain stays.
  return values
    .map((x) => {
      calls++;
      return x;
    })
    .map(() => calls);
}

function throws(): string {
  const values = [1, 2, 3];
  try {
    values
      .map((x) => note("t", x))
      .forEach((x) => {
        if (x === 1) throw new Error(`stop at ${x}`);
      });
    return "done";
  } catch (error) {
    return (error as Error).message;
  }
}

export function main(): void {
  console.log(twoEffects(), firstEffect());
  lastEffect();
  console.log(observer(), throws());
  console.log(log.join(" "));
}
