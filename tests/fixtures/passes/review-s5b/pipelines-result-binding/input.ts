// Review: the declared result is assigned once, at the end, so it stays in its temporal dead
// zone while the chain runs and keeps no partial value when a callback throws.

function tdz(): string {
  const xs = [1, 2];
  function peek(): number {
    return total;
  }
  try {
    const total = xs.reduce((sum, x) => sum + x + peek(), 0);
    return String(total);
  } catch (error) {
    return error instanceof ReferenceError ? "ReferenceError" : "other";
  }
}

function partialVar(): string {
  const xs = [10, 20, 30];
  try {
    // eslint-disable-next-line no-var
    var t = xs.reduce((sum, x) => {
      if (x === 30) throw new Error("stop");
      return sum + x;
    }, 0);
  } catch {
    // ignored
  }
  // eslint-disable-next-line no-var
  try {
    // eslint-disable-next-line no-var
    var r = xs.map((x) => {
      if (x === 30) throw new Error("stop");
      return x;
    }).map((v) => v);
  } catch {
    // ignored
  }
  return `${t!} ${r!}`;
}

function shadow(): number {
  const xs = [1, 2, 3];
  // A callback local named like the result is its own binding.
  const total = xs.map((x) => {
    const total = x * 2;
    return total;
  }).reduce((a, b) => a + b, 0);
  return total;
}

export function main(): void {
  console.log(tdz(), partialVar(), shadow());
}
