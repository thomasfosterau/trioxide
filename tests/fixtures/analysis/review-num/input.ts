function fr(x: number): number {
  return Math.fround(x);
}
function ops(a: number, b: number, s: string): number[] {
  const out: number[] = [];
  out.push(Math.fround(0.1));
  out.push(Math.fround(1e300));
  out.push(Math.fround(16777217) + 0);
  out.push(a * b);
  out.push(a / b);
  out.push(a % b);
  out.push(a - b);
  out.push(a ** b);
  out.push(Math.round(a));
  out.push(Math.floor(a));
  out.push(Math.ceil(a));
  out.push(Math.trunc(a));
  out.push(Math.sign(a));
  out.push(Math.sqrt(a));
  out.push(Math.abs(a));
  out.push(Math.max(a, b));
  out.push(Math.min(a, b));
  out.push(Math.max());
  out.push(Math.min());
  out.push(a | 0);
  out.push(a >>> 0);
  out.push(a >> b);
  out.push(a << b);
  out.push(~a);
  out.push(a & b);
  out.push(a ^ b);
  out.push(Math.round(Number(s)));
  out.push(parseInt(s));
  out.push(s.charCodeAt(5));
  out.push(Math.max(...out));
  out.push(Math.min(...[]));
  return out;
}
function inc(x: number): number {
  let y = x;
  y++;
  return y - x;
}
export function main(): void {
  console.log(fr(0.1), fr(3.4028235e38), fr(3.4028236e38));
  const xs = [0, -0, 0.5, -0.5, -0.4, 2.5, -2.5, 1e300, -1e300, Infinity, -Infinity, NaN, 2147483648, -2147483649, 4294967296, 9007199254740993, 1e-320, -1e-320, 3, -3, 31, 32];
  for (const a of xs) for (const b of xs) ops(a, b, "  -12.5e3 ");
  console.log(inc(9007199254740992), inc(1));
}
