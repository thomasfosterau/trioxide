// `typeof` checks prove a value's primitive type, even when TypeScript can't.

export function describe(value: unknown): string {
  if (typeof value === "number") {
    return "number " + (value /*@proven number*/);
  }
  if (typeof value === "string") {
    return "string " + value /*@proven string*/.length;
  }
  if (typeof value !== "boolean") {
    return "other";
  }
  return value /*@proven boolean*/ ? "yes" : "no";
}

function parse(input: string | number): number {
  // The declared union narrows to its number member; the check makes it proven.
  if (typeof input === "number") {
    return input /*@proven number*/;
  }
  return input /*@proven string*/.length;
}

export function main(): void {
  console.log(describe(1), describe("a"), describe(true), describe(null));
  console.log(parse(3), parse("abc"));
}
