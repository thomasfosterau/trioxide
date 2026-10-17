enum Direction {
  Up = 1,
  Down,
  Left,
  Right,
}

enum Level {
  Debug = "debug",
  Info = "info",
  Warn = "warn",
}

const enum Flags {
  None = 0,
  Read = 1 << 0,
  Write = 1 << 1,
  ReadWrite = Read | Write,
}

enum Computed {
  A = "abc".length,
  B = A * 2,
}

function describe(direction: Direction): string {
  switch (direction) {
    case Direction.Up:
    case Direction.Down:
      return "vertical";
    default:
      return "horizontal";
  }
}

export function main(): void {
  console.log(Direction.Down, Direction[3], describe(Direction.Left));
  console.log(Level.Warn, Object.values(Level).join("|"));
  const flags: Flags = Flags.ReadWrite;
  console.log(flags, (flags & Flags.Write) !== 0, Flags.None);
  console.log(Computed.A, Computed.B, Computed[6]);
}
