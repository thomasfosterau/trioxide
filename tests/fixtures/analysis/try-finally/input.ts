// Exceptions can leave a try block anywhere: a catch sees any value assigned in it.

function risky(n: number): number {
  if (n > 1) throw new Error("too big");
  return n;
}

export function main(): void {
  let stage: number | string = 0;
  try {
    stage = "started";
    risky(2);
    stage = 2;
  } catch {
    console.log(stage /*@proven union*/);
  } finally {
    console.log(stage /*@proven union*/);
  }
  let cleanup = false;
  try {
    risky(0);
  } finally {
    cleanup = true;
  }
  console.log(cleanup /*@proven boolean*/);
}
