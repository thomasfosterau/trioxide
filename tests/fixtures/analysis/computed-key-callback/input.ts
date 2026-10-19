// Functions stored under computed keys and called back through them: each one is called,
// with the arguments its caller passes, even though no call names it.
const registry: Record<string, (x: number) => number> = {};
function register(name: string, f: (x: number) => number): void {
  registry[name] = f;
}
function late1(x: number): number {
  return x * 2;
}
function late2(x: number): number {
  return x + 1;
}
function late3(x: number): number {
  return x - 1;
}
function late4(x: number): number {
  return x * 3;
}
function late5(x: number): string {
  if (typeof x === "number" && x > 0) return "positive";
  return x === 0 ? "zero" : "other";
}
const table: Record<string, (x: number) => number> = {};
const slots: ((x: number) => number)[] = [];
export function main(): void {
  register("a", late1);
  console.log(registry["a"]!(21));
  const key = "b" + String(slots.length);
  table[key] = late2;
  console.log(table.b0!(4));
  slots[slots.length] = late3;
  console.log(slots[0]!(10));
  const holder: Record<string, unknown> = {};
  holder[key] = late4;
  (globalThis as any).holder = holder;
  console.log(((globalThis as any).holder.b0 as (x: number) => number)(5));
  const named: Record<string, (x: number) => string> = {};
  named[key] = late5;
  console.log(named[key]!(3), named[key]!(0));
}
