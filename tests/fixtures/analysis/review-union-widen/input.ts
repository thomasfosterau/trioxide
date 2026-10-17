let s: number | null = null;

function step(): void {
  const v = s;
  s = v === null ? 0 : v + 1;
}

export function plain(): number {
  const a = 1 + 2;
  return a;
}

export function main(): void {
  for (let i = 0; i < 5; i++) step();
  console.log(s, plain());
}
