// Islands are synchronous.
async function later(n: number): Promise<number> {
  "use wasm";
  return n + 1;
}

export async function main(): Promise<void> {
  console.log(await later(1));
}
