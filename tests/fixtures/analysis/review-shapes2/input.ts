function make(): number {
  const o = { a: 1 };
  (o as any).b = 2;
  delete (o as any).a;
  return Object.keys(o).length;
}
export function main(): void {
  console.log(make());
}
