// Instances made at one `new` site of several constructors: methods, fields and shapes are
// those of whichever class (or function) ran, not of the first one the site saw.
class A {
  x = 1;
  m(): number {
    return 1;
  }
}
class B {
  x = "b";
  y = 2;
  m(): number {
    return 2;
  }
}
function F(this: any): void {
  this.z = 3;
}
const ctors: any[] = [A, B, F];
function make(i: number): A | B {
  const C = i === 0 ? A : B;
  return new C();
}
function makeAny(i: number): any {
  const C = ctors[i];
  return new C();
}
export function main(): void {
  const a = make(0);
  const b = make(1);
  const m = b.m();
  const x = b.x;
  console.log(a.m(), m, x, typeof a.x, Object.keys(b).join());
  (b as any).extra = true;
  console.log(Object.keys(b).join(), b instanceof B);
  const f = makeAny(2);
  const g = makeAny(1);
  console.log(f.z, g.y, f instanceof A, g instanceof B);
}
