// Replacing a method on a prototype changes what calls resolve to.

class Greeter {
  greet(): string {
    return "hello";
  }
}

(Greeter.prototype as { greet: () => unknown }).greet = () => 42;

export function main(): void {
  const greeter = new Greeter();
  // Either method may run.
  console.log(greeter.greet() /*@proven union*/);
}
