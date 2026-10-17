// Getters and setters run code on property access.

class Temperature {
  private celsius = 0;
  get fahrenheit(): number {
    return this.celsius * 1.8 + 32;
  }
  set fahrenheit(value: number) {
    this.celsius = (value - 32) / 1.8;
  }
}

export function main(): void {
  const t = new Temperature();
  t.fahrenheit = 212;
  console.log(t.fahrenheit /*@proven number*/);

  const counter = {
    count: 0,
    get next(): number {
      return ++this.count;
    },
  };
  // An object literal with a getter: reads run code trioxide treats as unknown.
  console.log(counter.next /*@declared number*/, counter.next);
}
