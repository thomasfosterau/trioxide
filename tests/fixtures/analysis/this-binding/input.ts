// `this` follows the call, not the definition.

class Counter {
  count = 0;
  increment(): number {
    return ++this.count;
  }
}

export function main(): void {
  const counter = new Counter();
    // `.bind` below hands the method to unknown code, so `this` is only declared.
  console.log(counter.increment() /*@declared number*/);
  const unbound = counter.increment;
  try {
    unbound();
  } catch {
    console.log("unbound");
  }
  const bound = counter.increment.bind(counter);
  console.log(bound() /*@declared number*/);
  const object = {
    name: "object",
    method(): string {
      return this.name /*@proven string*/;
    },
  };
  console.log(object.method());
}
