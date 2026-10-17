export function sideEffectCase(): string {
  let x: number | string = 1;
  let out = "";
  switch (2) {
    case ((x = "a"), 1):
      break;
    case 2: {
      const seen = x;
      out = String(seen);
    }
  }
  return out;
}

export function main(): void {
  console.log(sideEffectCase());
}
