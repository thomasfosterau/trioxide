export function finallyBreak(): string {
  let x: number | string = 1;
  while (true) {
    try {
      break;
    } finally {
      x = "after";
    }
  }
  const y = x;
  return String(y);
}

export function finallyContinue(): string {
  let x: number | string = 1;
  let out = "";
  for (let i = 0; i < 2; i++) {
    const seen = x;
    out += String(seen);
    try {
      continue;
    } finally {
      x = "s";
    }
  }
  return out;
}

export function main(): void {
  console.log(finallyBreak(), finallyContinue());
}
