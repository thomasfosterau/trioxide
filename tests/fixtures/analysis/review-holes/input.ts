function mapped(): number[] {
  const a: number[] = [1, 2, 3];
  a.length = 5;
  return a.map((x) => x * 2);
}
function sliced(): number[] {
  const a: number[] = new Array(3);
  a[0] = 1;
  return a.slice();
}
function found(): number | undefined {
  const a: number[] = [1, 2, 3];
  a.length = 5;
  let seen = 0;
  a.find((x) => { seen = x; return false; });
  return seen;
}
function idx(): number {
  const a = [1, 2, 3];
  a.length = 2;
  return a[2];
}
export function main(): void {
  const m = mapped();
  console.log(m[4], sliced()[1], found(), idx());
}
