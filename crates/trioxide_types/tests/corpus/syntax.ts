/**
 * A tour of expression and declaration syntax, for the span-correlation corpus.
 * Every construct here should line up between oxc and tsrs.
 */

// Leading line comment.
export interface Point {
  x: number;
  y: number;
  readonly label?: string;
}

export type Shape =
  | { kind: "circle"; radius: number }
  | { kind: "rect"; width: number; height: number };

export enum Color {
  Red,
  Green = 4,
  Blue = Green * 2,
}

export const enum Flag {
  A = 1 << 0,
  B = 1 << 1,
}

declare const external: { value: number };

/** JSDoc on a function. */
export function area(shape: Shape): number {
  switch (shape.kind) {
    case "circle":
      return Math.PI * shape.radius ** 2;
    case "rect":
      return shape.width * shape.height;
  }
}

export function overloaded(value: string): string;
export function overloaded(value: number): number;
export function overloaded(value: string | number): string | number {
  return typeof value === "string" ? value.toUpperCase() : value * 2;
}

export default function defaultExport(a: number, b = 2, ...rest: number[]): number {
  return a + b + rest.reduce((acc, next) => acc + next, 0);
}

export abstract class Animal {
  static count = 0;
  #secret = 1;
  protected abstract sound(): string;
  constructor(public readonly name: string, private legs: number = 4) {
    Animal.count++;
  }
  get description(): string {
    return `${this.name} has ${this.legs} legs and says ${this.sound()}`;
  }
  set nickname(value: string) {
    this.name.length > value.length ? void 0 : console.log(value);
  }
  static {
    Animal.count = 0;
  }
  reveal(): number {
    return this.#secret + (#secret in this ? 1 : 0);
  }
}

export class Dog extends Animal {
  protected sound(): string {
    return "woof";
  }
  override reveal(): number {
    return super.reveal() * 2;
  }
}

const dog = new Dog("rex");
const cat = new (class extends Animal {
  protected sound() {
    return "meow";
  }
})("tom", 3);

export function expressions(input: Point | null, list: readonly number[]): unknown[] {
  const p = input ?? { x: 0, y: 0 };
  const q = input?.x ?? -1;
  const label = input?.label?.trim() ?? "none";
  const nonNull = input!.y;
  const cast = (list as number[]).length;
  const angle = <number>(p.x as unknown);
  const sat = { x: 1, y: 2 } satisfies Point;
  const tuple = [1, "two", true] as const;
  const [first, , third = 3, ...others] = list;
  const { x, y: why = 0, ...restOfPoint } = p;
  const spread = [...list, ...[first, third]];
  const merged = { ...p, ...restOfPoint, z: 3, [`k${x}`]: why };
  const tagged = String.raw`a${x}b${y}c`;
  const regex = /ab+c/gi.test(label);
  const big = 10n ** 3n;
  const comma = (q, x, why);
  let counter = 0;
  counter += 1;
  counter **= 2;
  counter ||= 5;
  counter &&= 6;
  counter ??= 7;
  counter = counter++ + ++counter - counter-- - --counter;
  const bits = (counter | 0) & 0xff ^ (~counter >>> 1) << 2 >> 1;
  const cmp = counter < 1 || counter >= 2 && counter !== 3 && counter == 4;
  const kinds = [typeof counter, void counter, delete merged.z, "x" in p, dog instanceof Dog];
  const fn = function named(this: unknown, n: number) {
    return n;
  };
  const arrow = (n: number) => ({ n });
  const generic = <T,>(value: T): T => value;
  const optionalCall = (input as { call?: () => number } | null)?.call?.();
  const element = list[0]!;
  const computed = merged[`k${x}`];
  const chained = dog.description.toUpperCase().split(" ").map((w) => w.length);
  const conditional = cmp ? arrow(1).n : generic<number>(2);
  const parenthesized = ((counter));
  return [p, q, label, nonNull, cast, angle, sat, tuple, others, spread, merged, tagged, regex,
    big, comma, bits, kinds, fn(1), optionalCall, element, computed, chained, conditional,
    parenthesized, external.value, cat.name, Color.Blue, Flag.B];
}

export function* generator(limit: number): Generator<number, void, unknown> {
  for (let i = 0; i < limit; i++) {
    yield i;
  }
  yield* [limit];
}

export async function asyncWork(urls: string[]): Promise<number> {
  let total = 0;
  for await (const value of urls.map(async (url) => url.length)) {
    total += value;
  }
  try {
    await Promise.all(urls.map((url) => Promise.resolve(url)));
  } catch (error: unknown) {
    if (error instanceof Error) throw new Error(error.message, { cause: error });
  } finally {
    total--;
  }
  return total;
}

export function statements(values: Map<string, number>): number {
  let sum = 0;
  outer: for (const [key, value] of values) {
    for (const char in { a: 1 }) {
      if (char === key) continue outer;
    }
    while (sum < value) {
      sum++;
      if (sum > 100) break outer;
    }
    do {
      sum--;
    } while (sum > value);
  }
  if (sum) {
    sum = -sum;
  } else if (!sum) {
    sum = 1;
  } else sum = 2;
  label: {
    break label;
  }
  return sum;
}

namespace Geometry {
  export const origin: Point = { x: 0, y: 0 };
  export function distance(a: Point, b: Point = origin): number {
    return Math.hypot(a.x - b.x, a.y - b.y);
  }
}

export const meta = import.meta.url;
export const lazy = () => import("./other.js");
export const distance = Geometry.distance({ x: 3, y: 4 });
function target(this: void) {
  return new.target;
}
target();
