// A function handed to unknown code may be called with anything.

declare function register(handler: (value: number) => void): void;

function onValue(value: number): void {
  console.log(value /*@declared number*/);
}

function local(value: number): number {
  return value /*@proven number int*/ * 2;
}

export function main(): void {
  const handlers = [onValue];
  // Storing in a local array doesn't leak it...
  const first = handlers[0];
  // ...but handing it to an unknown function does.
  if (typeof register === "function") register(first);
  queueMicrotask(() => onValue(1));
  console.log(local(2));
}
