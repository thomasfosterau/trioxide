// Positive cases: `?.`, `??`, `typeof` and nullish checks the proven types decide.

interface Point {
  x: number;
  next?: Point;
}

function make(x: number): Point {
  return { x };
}

function chains(): unknown[] {
  const p = make(1);
  const f = (n: number): number => n + 1;
  const maybe = Math.random() > 2 ? null : p;
  // p and f are never nullish; maybe may be null.
  return [p?.x, f?.(1), p?.next?.x, maybe?.x];
}

function coalesce(): unknown[] {
  const zero = 0;
  const nothing = null;
  // `??` only replaces null and undefined.
  return [zero ?? 5, nothing ?? "fallback"];
}

function guards(value: number): string {
  if (typeof value !== "number") {
    return "not a number";
  }
  if (value === null || value === undefined) {
    return "nullish";
  }
  if (value == null) {
    return "loosely nullish";
  }
  return typeof value === "string" ? "string" : "number";
}

export function main(): void {
  console.log(chains(), coalesce(), guards(3), guards(4));
}
