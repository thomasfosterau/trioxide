// A class whose parent is only known once another function has run: by then its instance has
// escaped to code trioxide can't follow, which may call the new parent's methods on it.
class Base {
  describe(): string {
    return "base";
  }
}

class Other {
  tag = "other";
  describe(): string {
    return `${this.tag}!`;
  }
}

let Parent: typeof Base = Base;

function make(): Base {
  class Derived extends Parent {}
  return new Derived();
}

function useOther(): void {
  Parent = Other;
}

export function main(): void {
  const first = make();
  useOther();
  const second = make();
  // A key trioxide can't know: the call goes to whatever the objects have under it.
  const key = process.argv.length > 1000 ? "toString" : "describe";
  for (const object of [first, second]) {
    console.log((object as unknown as Record<string, () => string>)[key]());
  }
}
