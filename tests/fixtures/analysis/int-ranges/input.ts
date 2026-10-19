// Integer ranges for index arithmetic: a comparison that leaves no value makes its branch
// unreachable, products of integers that can't be -0 say so, and `i + k < n` bounds `i`.

function blur(src: Uint8Array, width: number, height: number): number {
  let sum = 0;
  for (let y = 0; y < height; y++) {
    for (let dy = -1; dy <= 1; dy++) {
      const ny = y + dy;
      if (ny >= 0 && ny < height) sum += src[(ny * width + 1) * 4];
    }
  }
  return sum;
}

function strided(xs: Uint8Array): number {
  let sum = 0;
  for (let i = 0; i + 3 < xs.length; i += 4) sum += xs[i] + xs[i + 3];
  for (let i = 1; i - 1 < xs.length; i += 2) sum += xs[i - 1];
  for (let i = 0; 2 + i <= xs.length; i += 3) sum += i;
  for (let i = 5; xs.length > i + 2; i += 5) sum += i;
  return sum;
}

function signs(a: number, b: number, z: number): number[] {
  const out: number[] = [];
  for (let i = -3; i <= 3; i++) {
    out.push(i * 4, i * -2, (i + 3) * 2, i * i);
  }
  out.push(z * 3, a * b, -0 * 4, 0 * -4);
  return out;
}

function edges(n: number, big: number, half: number): string {
  let parts = "";
  const two = 2;
  if (two > 2) parts += "unreachable";
  const minusZero = -0;
  if (minusZero < 0) parts += "unreachable";
  if (big + 3 < 10) parts += "big-small ";
  if (!(big + 3 < 10)) parts += "big-large ";
  if (n - 5 >= 0) parts += "n-ge ";
  else parts += "n-lt ";
  if (half + 1 < 2) parts += "half ";
  if (NaN + 1 < n) parts += "nan ";
  if (!(n + 1 < NaN)) parts += "not-nan ";
  return parts;
}

export function main(): void {
  const src = new Uint8Array(4 * 8 * 6);
  for (let i = 0; i < src.length; i++) src[i] = i % 7;
  console.log(blur(src, 8, 6));
  console.log(strided(src), strided(new Uint8Array(2)), strided(new Uint8Array(0)));
  console.log(signs(-1, 0, -0).map((v) => (Object.is(v, -0) ? "-0" : String(v))).join());
  console.log(edges(3, 2 ** 60, 0.5), edges(7, -(2 ** 60), 1.5));
}
