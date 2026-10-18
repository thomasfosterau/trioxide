// Review: an expression in the body may only be safe because the test passed; hoisting it into
// the head runs it even when the loop runs zero times.

interface Box {
  k: number;
}

function total(o: Box | undefined): number {
  let s = 0;
  for (let i = 0; o !== undefined && i < 3; i++) s += o.k * 2;
  return s;
}

let calls = 0;

function folded(): number {
  // The test folds to `false`: the body's `o.k * 2` must still never run.
  const o: Box | undefined = undefined;
  calls++;
  let s = calls - 1;
  for (let i = 0; o !== undefined && i < 3; i++) s += o.k * 2;
  return s;
}

export function main(): void {
  console.log(total(undefined), total({ k: 2 }), folded());
}
