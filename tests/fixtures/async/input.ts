const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

async function fetchValue(id: number): Promise<number> {
  await sleep(id % 3);
  return id * 10;
}

async function* countdown(from: number): AsyncGenerator<number, string, undefined> {
  for (let i = from; i > 0; i--) {
    await sleep(0);
    yield i;
  }
  return "liftoff";
}

function* fibonacci(): Generator<number> {
  let [a, b] = [0, 1];
  while (true) {
    yield a;
    [a, b] = [b, a + b];
  }
}

class Queue {
  private log: string[] = [];
  async run(tasks: Array<() => Promise<string>>): Promise<string[]> {
    for (const task of tasks) this.log.push(await task());
    return this.log;
  }
}

export async function main(): Promise<void> {
  console.log(await Promise.all([1, 2, 3].map(fetchValue)));
  const seen: number[] = [];
  for await (const n of countdown(3)) seen.push(n);
  console.log(seen.join(","));
  const fib: number[] = [];
  for (const n of fibonacci()) {
    if (n > 50) break;
    fib.push(n);
  }
  console.log(fib.join(" "));
  const results = await new Queue().run([async () => "a", () => sleep(1).then(() => "b")]);
  console.log(results);
  try {
    await Promise.reject(new Error("boom"));
  } catch (error) {
    console.log("caught", (error as Error).message);
  }
}
