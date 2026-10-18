// Review: a short-circuiting terminal must not skip the remaining calls, or a late throw, of
// an earlier stage that has effects.

function some(): string {
  const xs = [1, 2, 3, 4];
  const log: number[] = [];
  const r = xs.map((x) => { log.push(x); return x; }).some((y) => y === 2);
  return String(r) + log.join(",");
}

function every(): string {
  const xs = [1, 2, 3, 4];
  const log: number[] = [];
  const r = xs.filter((x) => { log.push(x); return true; }).every((y) => y < 2);
  return String(r) + log.join(",");
}

function find(): string {
  const xs = [1, 2, 3, 4];
  const log: number[] = [];
  const r = xs.filter((x) => { log.push(x); return x > 0; }).find((y) => y === 1);
  return String(r) + log.join(",");
}

function lateThrow(): string {
  const xs = [1, 2, 3];
  try {
    const r = xs.map((x) => { if (x === 3) throw new Error("late"); return x; }).some((y) => y === 1);
    return String(r);
  } catch (error) {
    return (error as Error).message;
  }
}

export function main(): void {
  console.log(some(), every(), find(), lateThrow());
}
