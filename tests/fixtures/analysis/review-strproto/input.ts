function patch(): void {
  (String.prototype as any).charCodeAt = () => "patched";
}
function use(s: string): number {
  return s.charCodeAt(0);
}
export function main(): void {
  patch();
  console.log(use("abc"));
}
