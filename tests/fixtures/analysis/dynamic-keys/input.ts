// Computed keys: written keys that can't be named make an object's contents unknown.

/** Uses a value without printing it (it depends on `Math.random()`). */
function sink(..._values: unknown[]): void {}

export function main(): void {
  const fixed = { a: 1, b: 2 };
  const key = "a";
  console.log(fixed[key] /*@proven number int*/);

  const bag: Record<string, number> = { a: 1 };
  const dynamic = Math.random() > 0.5 ? "a" : "b";
  bag[dynamic] = 5;
  sink(bag.a /*@declared number*/);

  const list = [1, 2, 3];
  const i = Math.floor(Math.random() * 3);
  list[i] = 10;
  sink(list[0] /*@proven union*/);
}
