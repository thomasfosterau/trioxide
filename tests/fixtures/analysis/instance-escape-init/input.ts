// A class without a constructor, escaped to unknown code, whose field initialiser hands `this`
// to unknown code: the instance escapes, so unknown code may add properties to it and to
// what it holds.
(globalThis as any).sink = (target: any): number => {
  (globalThis as any).held = target;
  return 1;
};
class Holder {
  n = (globalThis as any).sink([this]);
  data: { x: number } = { x: 0 };
}
(globalThis as any).escaped = Holder;
const holder = new Holder();
holder.data = { x: 1 };
export function main(): void {
  (globalThis as any).held[0].extra = "added";
  (globalThis as any).held[0].data.y = "added by unknown code";
  console.log(holder.n, "extra" in holder);
}
