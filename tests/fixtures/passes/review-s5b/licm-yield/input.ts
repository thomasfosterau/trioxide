// Review: other code can resize an array while a generator is suspended at a `yield`.

function* walk(a: number[]): Generator<number> {
  for (let i = 0; i < a.length; i++) {
    yield a[i];
  }
}

export function main(): void {
  const a = [1, 2, 3];
  const seen: number[] = [];
  for (const x of walk(a)) {
    seen.push(x);
    if (x === 1) a.pop();
  }
  console.log(seen.join(","));
}
