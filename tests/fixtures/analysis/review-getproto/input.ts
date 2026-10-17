function patch(): void {
  const proto: any = Object.getPrototypeOf([]);
  proto.indexOf = () => "patched";
}
function use(a: number[]): number {
  return a.indexOf(3);
}
export function main(): void {
  patch();
  console.log(use([1, 2, 3]));
}
