// Adversarial: a program that subclasses `Array` and gives an array its own `map`. Using
// `Array` as a value (`extends Array`) turns the builtin models off for the whole program,
// so nothing here becomes a loop.

class Bag<T> extends Array<T> {}

function subclass(): number[] {
  // A subclass's `map` makes a `Bag` (species): not a plain array.
  const bag = Bag.from([1, 2, 3]) as Bag<number>;
  return bag.map((x) => x + 1);
}

function ownMethod(): number[] {
  const a = [1, 2];
  (a as unknown as { map: () => number[] }).map = () => [42];
  return a.map((x) => x * 2);
}

function plain(): number {
  return [1, 2, 3].reduce((s, x) => s + x, 0);
}

export function main(): void {
  console.log(subclass(), subclass() instanceof Bag, ownMethod(), plain());
}
