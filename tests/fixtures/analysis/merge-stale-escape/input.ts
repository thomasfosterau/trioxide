// A subclass made in one function escapes in another; the parent it gets is only known once
// a third has run. When those writes are merged from different chunks of a sweep, the
// parent's escape must survive the merge: unknown code changes `Base1.tag` through it.
class Base0 {
  static tag = 0;
}
class Base1 {
  static tag = 1;
}
let made: unknown;
function a(): void {
  (globalThis as any).leak = made;
}
function b(base: typeof Base0): void {
  made = class Sub extends base {};
}
function d(v: number): void {
  Base1.tag = v;
}
function later(): void {
  b(Base1);
  d(3);
}
export function main(): void {
  b(Base0);
  d(2);
  later();
  a();
  const leaked = (globalThis as any)["le" + "ak"];
  Object.getPrototypeOf(leaked).tag = "changed by unknown code";
  console.log(Base1.tag);
}
