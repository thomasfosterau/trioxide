// Arithmetic that throws on bigints is not inert, however literal it looks.
function attempt(name: string, run: () => void): string {
  try {
    run();
    return `${name}: ok`;
  } catch (error) {
    return `${name}: ${(error as Error).name}`;
  }
}
export function main(): void {
  console.log(
    attempt("plus", () => {
      // @ts-ignore
      +(1n);
    }),
    attempt("mixed", () => {
      // @ts-ignore
      -(-1n) + 1;
    }),
    attempt("negate", () => {
      -(-1n);
    }),
    -(1n),
    ~1n,
  );
}
