function known(n: number): number {
  return n + 1;
}
export function main(): void {
  const f: (n: number) => number = Math.random() < 2 ? known : (globalThis as any).other;
  console.log(f(1));
}
