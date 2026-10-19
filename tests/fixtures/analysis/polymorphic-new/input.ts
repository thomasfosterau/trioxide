// One `new` site constructs `A` or `B`: the instance escapes, and unknown code reaches its
// class through `.constructor`, so `B.tag` is not the `1` it was declared with.
class A {
  static tag: unknown = 0;
}
class B {
  static tag: unknown = 1;
}
function pick(flag: boolean): typeof A {
  return flag ? A : B;
}
let inst: unknown;
function make(flag: boolean): void {
  const C = pick(flag);
  inst = new C();
}
export function main(): void {
  make(true);
  make(false);
  (globalThis as any).leak = inst;
  (globalThis as any)["le" + "ak"].constructor.tag = "changed by unknown code";
  const t = B.tag;
  console.log(t);
}
