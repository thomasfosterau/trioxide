// App mode: every caller of a module-private function is visible, so its parameters are
// what the calls pass. (The entry's own exports are open: the host may call them.)

function scale(value: number, factor: number): number {
  return value /*@proven number int*/ * factor /*@proven number int*/;
}

const shared = { count: 0 };

function bump(): number {
  shared.count++;
  return shared.count /*@proven number*/;
}

function escaping(value: number): number {
  return value /*@declared number*/ + 1;
}

export function main(): void {
  console.log(scale(2, 3), scale(4, 5), bump());
  // Handing the function to unknown code means anyone may call it.
  [1, 2].forEach(escaping);
  setTimeout(escaping, 0);
}
