// Adversarial: inlined callbacks share one loop body, so their bindings must not capture each
// other's names, the loop's, or the receiver's.

function shadows(): number {
  const x = 10;
  const values = [1, 2, 3, 4, 5, 6];
  // The `map` parameter `x` would hide the outer `x` the `filter` reads.
  return values.map((x) => x * 2).filter((y) => y > x).reduce((sum, y) => sum + y, 0);
}

function receiverName(): number[] {
  const values = [1, 2];
  // A parameter named like the receiver.
  return values.map((values) => values * 3);
}

function indexName(): number[] {
  const i = 100;
  const values = [1, 2];
  // The callback reads an outer `i`: the loop's index can't be called that.
  return values.map((v) => v + i);
}

function sameNames(): number {
  const values = [3, 4, 5];
  // Every callback calls its element `v`, and declares a `t`.
  return values
    .map((v) => {
      const t = v * 2;
      return t;
    })
    .filter((v) => {
      const t = v > 6;
      return t;
    })
    .reduce((acc, v) => {
      const t = acc + v;
      return t;
    }, 0);
}

function captured(): number[] {
  const makers: (() => number)[] = [];
  const values = [1, 2, 3];
  // Closures in the callback capture each element's own binding.
  values.forEach((v) => {
    makers.push(() => v * 10);
  });
  return makers.map((make) => make());
}

function accumulatorCaptured(): number {
  const values = [1, 2, 3];
  const seen: (() => number)[] = [];
  // A closure captures the accumulator parameter: it keeps its own binding.
  const total = values.reduce((acc, v) => {
    seen.push(() => acc);
    return acc + v;
  }, 0);
  return total * 100 + seen.map((f) => f()).reduce((a, b) => a + b, 0);
}

function reassignedParam(): number[] {
  const values = [1, 2, 3];
  // The callback reassigns its parameter, which then can't stand for the element.
  return values
    .filter((v) => {
      v = v * 10;
      return v > 10;
    })
    .map((v) => v + 1);
}

export function main(): void {
  console.log(shadows(), receiverName(), indexName(), sameNames());
  console.log(captured(), accumulatorCaptured(), reassignedParam());
}
