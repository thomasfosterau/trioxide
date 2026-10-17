import {
  addSelfInc,
  callArgs,
  compoundCallIndex,
  compoundIndexThenInc,
  incIndexThenRead,
  mulAssignInc,
  plusSelfInc,
  storeCallIndex,
  storeIndexThenInc,
} from "./kernel.js";

export function main(): void {
  for (const a of [0, 5, -2.5]) {
    console.log(addSelfInc(a), plusSelfInc(a), mulAssignInc(a));
  }
  const xs = new Float64Array([1, 2]);
  console.log(storeIndexThenInc(xs), xs.join(","));
  const ys = new Float64Array([1, 2]);
  console.log(compoundIndexThenInc(ys), ys.join(","));
  const zs = new Float64Array([1, 2]);
  console.log(incIndexThenRead(zs), zs.join(","));
  const log = new Float64Array(1);
  const ws = new Float64Array(3);
  storeCallIndex(ws, log);
  console.log(ws.join(","), log[0]);
  log[0] = 0;
  compoundCallIndex(ws, log);
  console.log(ws.join(","), log[0]);
  console.log(callArgs(log), log[0]);
}
