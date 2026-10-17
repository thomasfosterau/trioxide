export function helper(n: number): number {
  return n * 2;
}

export function main(flag?: number): void {
  const v = flag;
  console.log(v, helper(3));
}
