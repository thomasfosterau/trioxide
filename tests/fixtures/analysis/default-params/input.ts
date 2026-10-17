// Default parameters replace missing (undefined) arguments.

function greet(name = "world", times: number = 1): string {
  return (name /*@proven string*/).repeat(times /*@proven number int*/);
}

function optional(value?: number): number {
  return value ?? 0;
}

export function main(): void {
  console.log(greet(), greet("you"), greet("twice", 2));
  console.log(optional() /*@proven number int*/, optional(3));
}
