// Escape facts: where allocated objects may end up.

let global: { v: number } | undefined;

function keep(o: { v: number }): void {
  global = o;
}

function read(o: { v: number }): number {
  return o.v;
}

export function main(): void {
  const local = { v: 1 } /*@proven object escape=none*/;
  const passed = { v: 2 } /*@proven object escape=known*/;
  const stored = { v: 3 } /*@proven object escape=known*/;
  const leaked = { v: 4 } /*@proven object escape=unknown*/;
  console.log(local.v + read(passed));
  keep(stored);
  console.log(leaked);
  const pair = [1, 2] /*@proven array escape=none*/;
  const [a, b] = pair;
  console.log(a + b, global?.v);
}
