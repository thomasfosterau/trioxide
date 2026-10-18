// Branchy integer-valued scalar code, called once: no arrays.
export function collatz(limit: number): number {
  let best = 0;
  for (let n = 1; n < limit; n++) {
    let x = n;
    let steps = 0;
    while (x !== 1) {
      x = x % 2 === 0 ? x / 2 : 3 * x + 1;
      steps++;
    }
    if (steps > best) best = steps;
  }
  return best;
}
