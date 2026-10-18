// Negative cases: only TypeScript declares these values (library world: exported functions
// may be called with anything), so nothing is inlined and the skips are logged.

export enum Unsealed {
  A = 1,
  B = 2,
}

export function scaled(factor: 3): number {
  return factor * 2;
}

export function member(): number {
  // An exported enum in the open world may be changed by its importers.
  return Unsealed.B;
}

export function main(): void {
  console.log(scaled(3), member());
}
