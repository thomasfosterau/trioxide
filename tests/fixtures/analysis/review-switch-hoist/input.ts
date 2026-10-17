export function pick(k: number): number | string {
  switch (k) {
    case 1:
      return g();
    case 2: {
      break;
    }
    default:
      function g(): string {
        return "from g";
      }
  }
  return 0;
}

export function main(): void {
  const r = pick(1);
  console.log(r);
}
