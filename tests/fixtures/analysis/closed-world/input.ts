// App mode: every caller is visible, so parameters are what the calls pass.

export function scale(value: number, factor: number): number {
  return value /*@proven number int*/ * factor /*@proven number int*/;
}

export const shared = { count: 0 };

export function bump(): number {
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
