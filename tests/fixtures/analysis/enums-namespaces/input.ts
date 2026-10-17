// Enums and namespaces are objects at run time; their members come from TypeScript's types.

enum Direction {
  Up = 1,
  Down,
}

const enum Flags {
  A = 1,
  B = 2,
}

namespace Units {
  export const meter = 1;
  export function km(value: number): number {
    return value * 1000;
  }
}

export function main(): void {
  console.log(Direction.Down /*@declared number*/, Direction[1]);
  console.log(Flags.B /*@declared number*/);
  console.log(Units.km(2) /*@declared number*/, Units.meter);
}
