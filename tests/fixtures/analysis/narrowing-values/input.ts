// Equality and truthiness narrowing on locals.

/** Uses a value without printing it (it depends on `Math.random()`). */
function sink(..._values: unknown[]): void {}

const config: { level?: number } | undefined = JSON.parse("null");

export function main(): void {
  const items: (string | null)[] = ["a", null];
  for (const item of items) {
    if (item === null) {
      console.log(item /*@proven null*/);
    } else {
      console.log(item /*@proven string*/);
    }
  }

  let count = Math.floor(Math.random() * 3);
  if (count) {
    // Truthy: not 0 (and not NaN).
    sink(count /*@proven number int nonNaN*/);
  }
  if (count === 2) {
    sink(count /*@proven number int*/);
  }
  count = count > 1 ? count : 1;

  const level = config?.level ?? 0;
  console.log(level /*@declared number*/);
  const flag = Math.random() > 0.5 ? "on" : undefined;
  if (flag == null) {
    sink(flag /*@proven undefined*/);
  } else {
    sink(flag /*@proven string*/);
  }
}
