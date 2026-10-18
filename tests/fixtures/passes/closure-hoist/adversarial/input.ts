// Closures whose identity can be seen, or that capture something, stay where they are.
const cache = new WeakMap<object, number>();
const keep: Array<() => number> = [];

export function main(): void {
  const results: string[] = [];
  let sum = 0;
  for (let i = 0; i < 3; i++) {
    // Hoisted: only called, captures nothing.
    const one = (): number => 1;
    sum += one();

    const compared = (): number => 2;
    const fresh = compared === keep[0];
    const keyed = (): number => 3;
    cache.set(keyed, i);
    const tagged = (): number => 4;
    (tagged as unknown as { calls: number }).calls = ((tagged as unknown as { calls?: number }).calls ?? 0) + 1;
    const pushed = (): number => 5;
    keep.push(pushed);
    const captures = (): number => i;
    const self = function (this: unknown): unknown { return this; };
    const early = function () { return late(); };
    const late = (): number => 6;
    results.push(
      [fresh, cache.has(keyed), (tagged as unknown as { calls: number }).calls, captures(), early(), typeof self].join(" "),
    );
  }
  console.log(sum, keep.length, keep[0] === keep[1], results.join("|"));
}
