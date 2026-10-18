// A `Q` made at a `new` site that also makes `P`s gets a late key, and reaches unknown code
// (`console.log` prints its keys): `Q`'s shape is not stable, and the key stays late.
class P {
  a = 1;
}
class Q {
  a = 2;
}
function make(flag: boolean): P | Q {
  const C = flag ? P : Q;
  return new C();
}
export function main(): void {
  const p = make(true);
  const q = make(false) as Q & { b?: number };
  if (Number("1") === 1) q.b = 3;
  console.log(p.a, q);
}
