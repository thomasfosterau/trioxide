function patch(): void {
  const p: any = Object.getPrototypeOf(Object.getPrototypeOf([]));
  p[1] = "hole";
}
function read(): number | undefined {
  const a = [1, , 3];
  return a[1];
}
function inbounds(): number {
  const a: number[] = [1, , 3] as number[];
  let t = 0;
  for (let i = 0; i < a.length; i++) {
    const v = a[i];
    t = v;
  }
  return t;
}
export function main(): void {
  patch();
  console.log(read(), inbounds());
}
