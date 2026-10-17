// JS evaluates an assignment's target (array, then index, or the variable's old value)
// before its right-hand side.
export function addSelfInc(a: number): number {
  "use wasm";
  let y = a;
  y += y++;
  return y;
}

export function plusSelfInc(a: number): number {
  "use wasm";
  let y = a;
  y = y + y++;
  return y;
}

export function mulAssignInc(a: number): number {
  "use wasm";
  let y = a;
  y *= y++ + 1;
  return y;
}

export function storeIndexThenInc(xs: Float64Array): number {
  "use wasm";
  let i = 0;
  xs[i] = i++ + 10;
  return i;
}

export function compoundIndexThenInc(xs: Float64Array): number {
  "use wasm";
  let i = 0;
  xs[i] += i++ + 10;
  return i;
}

export function incIndexThenRead(xs: Float64Array): number {
  "use wasm";
  let i = 0;
  xs[i++] = i + 20;
  return i;
}

function bump(log: Float64Array, v: number): number {
  log[0] = log[0] * 100 + v;
  return v;
}

export function storeCallIndex(xs: Float64Array, log: Float64Array): void {
  "use wasm";
  xs[bump(log, 1)] = bump(log, 2);
}

export function compoundCallIndex(xs: Float64Array, log: Float64Array): void {
  "use wasm";
  xs[bump(log, 1)] += bump(log, 2);
}

export function callArgs(log: Float64Array): number {
  "use wasm";
  let k = 1;
  return pair(k, k++) + pair(k++, k) * 100 + bump(log, 3) * 0;
}

function pair(a: number, b: number): number {
  return a * 10 + b;
}
