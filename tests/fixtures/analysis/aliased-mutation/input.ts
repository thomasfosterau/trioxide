// Writes through an alias must reach every reader of the same object.

function poke(target: { x: number | string }): void {
  target.x = "poked";
}

export function main(): void {
  const a = { x: 1 };
  const b: { x: number | string } = a;
  b.x = "changed";
  console.log(a.x /*@proven union*/);

  const c = { x: 1 };
  poke(c);
  console.log(c.x /*@proven union*/);

  const list = [1, 2, 3];
  const alias: (number | string)[] = list;
  alias.push("four");
  const last = list[3];
  console.log(last /*@proven union*/);

  // An object whose only writer is local keeps its exact shape.
  // Logging hands the object to unknown code. The heap is flow-insensitive, so even the read
  // before the call only has the declared type.
  const untouched = { y: 2 };
  const y = untouched.y;
  console.log(untouched /*@proven object*/, y /*@declared number*/);
  // An object nobody else sees keeps its exact shape.
  const exact = { w: 4 };
  const w = exact.w + (exact /*@proven object exact*/).w;
  console.log(w);

  const box = { inner: { v: 1 } };
  const inner = box.inner;
  inner.v = 2;
  console.log(box.inner.v /*@proven number int*/);
}
