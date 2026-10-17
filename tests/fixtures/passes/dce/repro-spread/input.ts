// S2 repro: spreading an object runs its getters, so the unused copy (and the call) must stay.
let log = 0;
function viaSpread(): number {
  const source = {
    get counted(): number {
      log += 10;
      return 1;
    },
  };
  const copy = { ...source };
  return 0;
}
export function main(): void {
  viaSpread();
  console.log(log);
}
