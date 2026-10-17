interface Comparable<T> {
  compareTo(other: T): number;
}

class Version implements Comparable<Version> {
  constructor(
    readonly major: number,
    readonly minor: number,
  ) {}

  compareTo(other: Version): number {
    return this.major - other.major || this.minor - other.minor;
  }

  toString(): string {
    return `${this.major}.${this.minor}`;
  }
}

function max<T extends Comparable<T>>(items: readonly T[]): T | undefined {
  let best: T | undefined;
  for (const item of items) {
    if (best === undefined || item.compareTo(best) > 0) best = item;
  }
  return best;
}

class Stack<T> {
  private items: T[] = [];
  push(...items: T[]): this {
    this.items.push(...items);
    return this;
  }
  pop(): T | undefined {
    return this.items.pop();
  }
  get size(): number {
    return this.items.length;
  }
}

type Pair<A, B = A> = readonly [A, B];
const swap = <A, B>([a, b]: Pair<A, B>): Pair<B, A> => [b, a];

function groupBy<T, K extends PropertyKey>(items: T[], key: (item: T) => K): Record<K, T[]> {
  const groups = {} as Record<K, T[]>;
  for (const item of items) (groups[key(item)] ??= []).push(item);
  return groups;
}

export function main(): void {
  const versions = [new Version(1, 2), new Version(2, 0), new Version(1, 10)];
  console.log(String(max(versions)), max<Version>([]));
  const stack = new Stack<string>().push("a", "b", "c");
  console.log(stack.pop(), stack.size);
  console.log(swap([1, "one"] as const).join(" "));
  const config = { retries: 3, verbose: true } satisfies Record<string, number | boolean>;
  console.log(config.retries + 1, JSON.stringify(groupBy([1, 2, 3, 4, 5], (n) => (n % 2 ? "odd" : "even"))));
}
