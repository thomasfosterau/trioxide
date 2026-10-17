function lenAssign(a: number[]): number {
  let t = 0;
  for (let i = 0; i < a.length; i++) { a.length = 0; t += a[i] ?? 0; }
  return t;
}
function alias(a: number[]): number {
  const b = a;
  let t = 0;
  for (let i = 0; i < a.length; i++) { b.pop(); t += a[i] ?? 0; }
  return t;
}
function splice(a: number[]): number {
  let t = 0;
  for (let i = 0; i < a.length; i++) { a.splice(0, 1); t += a[i] ?? 0; }
  return t;
}
function modI(a: number[]): number {
  let t = 0;
  for (let i = 0; i < a.length; i++) { i += 1; t += a[i] ?? 0; }
  return t;
}
function le(a: number[]): number {
  let t = 0;
  for (let i = 0; i <= a.length; i++) { t += a[i] ?? 0; }
  return t;
}
function frac(a: number[]): number {
  let t = 0;
  for (let i = 0.5; i < a.length; i++) { t += a[i] ?? 0; }
  return t;
}
function neg(a: number[]): number {
  let t = 0;
  for (let i = -2; i < a.length; i++) { t += a[i] ?? 0; }
  return t;
}
let shared: number[] = [1, 2, 3];
function shrinkShared(): void { shared.pop(); }
function viaCall(): number {
  let t = 0;
  for (let i = 0; i < shared.length; i++) { shrinkShared(); t += shared[i] ?? 0; }
  return t;
}
function cb(a: number[]): number {
  let t = 0;
  for (let i = 0; i < a.length; i++) { [0].forEach(() => { a.pop(); }); t += a[i] ?? 0; }
  return t;
}
function str(s: string): number {
  let t = 0;
  for (let i = 0; i < s.length; i++) { t += s.charCodeAt(i); const c = s[i]; t += c.length; }
  return t;
}
function detach(a: Float64Array): number {
  let t = 0;
  for (let i = 0; i < a.length; i++) { if (i === 1) structuredClone(a.buffer, { transfer: [a.buffer] }); t += a[i] ?? 0; }
  return t;
}
function sub(a: Float64Array): number {
  let t = 0;
  const s = a.subarray(0, 2);
  for (let i = 0; i < a.length; i++) { t += s[i] ?? 0; }
  return t;
}
function whileLoop(a: number[]): number {
  let t = 0; let i = 0;
  while (i < a.length) { t += a[i] ?? 0; i++; a.shift(); }
  return t;
}
function reassign(a: number[]): number {
  let t = 0;
  for (let i = 0; i < a.length; i++) { a = []; t += a[i] ?? 0; }
  return t;
}
function setLen(a: number[]): number {
  let t = 0; const o = { a };
  for (let i = 0; i < a.length; i++) { o.a.length = 0; t += a[i] ?? 0; }
  return t;
}
export function main(): void {
  console.log(lenAssign([1, 2, 3]), alias([1, 2, 3]), splice([1, 2, 3]), modI([1, 2, 3]), le([1, 2, 3]),
    frac([1, 2, 3]), neg([1, 2, 3]), viaCall(), cb([1, 2, 3]), str("abc"), sub(new Float64Array(4)),
    whileLoop([1, 2, 3]), reassign([1, 2]), setLen([1, 2]), detach(new Float64Array(3)));
}
