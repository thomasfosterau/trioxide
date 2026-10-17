function patch(): void {
  const g: any = Function("return this")();
  g.Math.floor = () => "patched";
}
function use(x: number): number {
  return Math.floor(x);
}
export function main(): void {
  patch();
  console.log(use(1.5));
}
