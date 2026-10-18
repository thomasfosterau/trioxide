// Adversarial: hoisting must not run code a zero-trip loop wouldn't, read a length the loop
// changes, or skip a narrowing the loop's test provides.

interface Box {
  value: number;
}

function narrowed(box: Box | null, n: number): number {
  let total = 0;
  // `box.value` is only safe once the test has passed: it reads what the test narrows.
  for (let i = 0; box !== null && i < n; i++) total += box.value;
  return total;
}

function grows(): number {
  const a = [1, 2];
  let steps = 0;
  // The loop pushes onto `a`: its length is read every time.
  for (let i = 0; i < a.length && i < 6; i++) {
    a.push(i);
    steps++;
  }
  return steps;
}

function splices(): number {
  const a = [1, 2, 3, 4, 5];
  let steps = 0;
  // `splice` resizes in a way the analysis doesn't attribute to a site.
  for (let i = 0; i < a.length; i++) {
    a.splice(i, 1);
    steps++;
  }
  return steps;
}

function truncates(): number {
  const a = [1, 2, 3, 4];
  let steps = 0;
  for (let i = 0; i < a.length; i++) {
    if (i === 1) a.length = 2;
    steps++;
  }
  return steps;
}

let calls = 0;

function expensive(x: number): number {
  calls++;
  return x * 2;
}

function zeroTrip(n: number, x: number): number {
  let total = 0;
  // `expensive(x)` is a program call: a loop that doesn't run mustn't start calling it.
  for (let i = 0; i < n; i++) total += expensive(x);
  return total;
}

function head(): number {
  const a = [1, 2, 3];
  let total = 0;
  // The head runs before anything put before the loop, and has effects.
  for (const v of shrink(a)) total += v * a.length;
  return total;
}

function shrink(a: number[]): number[] {
  a.pop();
  return [10, 20];
}

export function main(): void {
  console.log(narrowed(null, 3), narrowed({ value: 2 }, 3), grows(), splices(), truncates());
  console.log(zeroTrip(0, 1), calls, zeroTrip(2, 1), calls, head());
}
