// Review: other code can resize an array while a loop is suspended at an `await`.

async function sum(a: number[]): Promise<number> {
  let s = 0;
  for (let i = 0; i < a.length; i++) {
    s += a[i];
    await null;
  }
  return s;
}

export async function main(): Promise<void> {
  const a = [1, 2, 3];
  const pending = sum(a);
  a.pop();
  console.log(await pending);
}
