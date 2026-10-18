// A subclass instance that escapes to code trioxide can't follow takes its inherited methods
// with it: unknown code may call `Base.prototype.describe` on it, so `describe` is open.
class Base {
  label = "base";
  describe(): string {
    return `${this.label}:${this.kind()}`;
  }
  kind(): string {
    return "base";
  }
}

class Derived extends Base {
  kind(): string {
    return "derived";
  }
}

export function main(): void {
  const derived = new Derived();
  // A key trioxide can't know: the call goes to whatever `derived` has under it.
  const key = process.argv.length > 1000 ? "kind" : "describe";
  console.log((derived as unknown as Record<string, () => string>)[key]());
}
