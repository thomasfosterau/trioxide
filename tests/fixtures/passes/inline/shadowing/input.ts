// Values whose names are shadowed where they land: `Infinity`, `NaN` and `undefined` are
// parameters, so the literals are written `1 / 0`, `0 / 0` and `void 0`.
const NOTHING = undefined;

function shadowed(Infinity: number, NaN: number, undefined: number): unknown[] {
  // An inlined constant, and syntactic folds (in the cleanup group).
  return [NOTHING, 2 / 0, -2 / 0, +"x", Infinity, NaN, undefined];
}

function unshadowed(): unknown[] {
  return [NOTHING, 2 / 0, -2 / 0, +"x"];
}

export function main(): void {
  console.log(shadowed(1, 2, 3), shadowed(4, 5, 6), unshadowed());
}
