// Generators and async functions: their results are generator objects and promises.

function* range(limit: number): Generator<number> {
  for (let i = 0; i < limit; i++) yield i;
}

async function delayed(value: number): Promise<number> {
  await null;
  return value * 2;
}

export async function main(): Promise<void> {
  let total = 0;
  for (const value of range(3)) {
    total += value /*@declared number*/;
  }
  const result = await delayed(total);
  console.log(total, result /*@declared number*/);
}
