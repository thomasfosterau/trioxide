// Optional chaining and nullish coalescing.

type Node = { value: number; next?: Node };

function second(node: Node | undefined): number | undefined {
  return node?.next?.value;
}

export function main(): void {
  const list: Node = { value: 1, next: { value: 2 } };
  const value = second(list);
  console.log(value /*@proven union*/, second(undefined));
  const fallback = value ?? -1;
  console.log(fallback /*@proven number int*/);
  const maybe = Math.random() > 2 ? list : undefined;
  console.log(maybe?.value /*@proven union*/);
}
