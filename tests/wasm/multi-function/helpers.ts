// Plain helpers in another module: compiled into the island as callees, and still exported
// as JS.

export function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

export function smooth(t: number): number {
  return t * t * (3 - 2 * t);
}
