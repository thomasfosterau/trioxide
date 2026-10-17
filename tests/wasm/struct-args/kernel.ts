"use wasm";
// Object arguments: struct types are told apart by declaration (`Ä` and `Ö` mangle to the
// same name, as do the parameter types of `move_p(x)` and `move(p_x)`), fields are written
// back only when the island wrote them, and objects whose fields aren't plain writable data
// properties (frozen, getters) run the JS version.

export interface Ä {
  x: number;
}

export interface Ö {
  y: number;
}

export interface P {
  x: number;
  flag: boolean;
}

export function bumpX(a: Ä): void {
  a.x = a.x + 1;
}

export function bumpY(b: Ö): void {
  b.y = b.y + 1;
}

export function move_p(x: { k: number }): void {
  x.k = x.k + 1;
}

export function move(p_x: { j: number }): void {
  p_x.j = p_x.j + 1;
}

export function shift(p: P, n: number): void {
  if (n > 0) p.x = p.x + n;
  if (p.x > 100) p.flag = !p.flag;
}
