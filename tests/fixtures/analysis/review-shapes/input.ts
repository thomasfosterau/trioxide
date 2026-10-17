class P {
  x = 1;
}
class Q {
  x = 1;
  grow(): void {
    (this as any).y = 2;
  }
}
export function main(): void {
  const p = new P();
  (p as any).z = 3;
  const q = new Q();
  q.grow();
  const o = { a: 1 };
  (o as any).b = 2;
  console.log(Object.keys(p).length, Object.keys(q).length, Object.keys(o).length);
}
