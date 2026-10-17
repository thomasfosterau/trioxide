function patch(): void {
  (Number.prototype as any).toFixed = () => 42;
}
function use(n: number): string {
  return n.toFixed(2);
}
export function main(): void {
  patch();
  console.log(use(1.5));
}
