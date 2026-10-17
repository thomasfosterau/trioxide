// The negative cases with trustTypes: declared facts count as proof, so they inline.

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
