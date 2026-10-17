// `any` launders values past the type checker: the facts must follow values, not annotations.

function fromAny(): any {
  return "not a number";
}

export function main(): void {
  const a: any = "text";
  const n: number = a;
  // The value is the string literal, whatever the annotation says.
  console.log(n /*@proven string*/);

  const b: number = fromAny();
  // An unknown call's `any` result is unknown, and the annotation can't make it proven.
  console.log(b /*@proven string*/, typeof b);

  const json = JSON.parse("[1, 2]");
  const first: number = json[0];
  // Only the annotation says it is a number.
  console.log(first /*@declared number*/);

  const laundered = (json as unknown) as number[];
  console.log(laundered /*@declared array*/, laundered.length /*@declared number*/);
}
