function patch(): void {
  (Uint8Array.prototype as any).indexOf = () => "patched";
}
function use(a: Uint8Array): number {
  return a.indexOf(3);
}
export function main(): void {
  patch();
  console.log(use(new Uint8Array([1, 2, 3])));
}
