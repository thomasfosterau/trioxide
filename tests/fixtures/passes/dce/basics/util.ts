// A non-entry module: exports no module imports are dropped (app world), then the bindings.
export function used(x: number): number {
  return x + 1;
}

export function unusedHelper(x: number): number {
  return x * 3;
}

export const UNUSED_TABLE = [1, 2, 3];

const internal = 4;
export { internal };
