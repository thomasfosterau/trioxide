export function catchThenFinally(): string {
  let x: number | string = 1;
  let out = "";
  try {
    try {
      throw new Error("a");
    } catch {
      x = "s";
      throw new Error("b");
    } finally {
      const seen = x;
      out = String(seen);
    }
  } catch {
    // swallowed
  }
  return out;
}

export function main(): void {
  console.log(catchThenFinally());
}
