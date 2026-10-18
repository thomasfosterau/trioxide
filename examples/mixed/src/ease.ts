// A boundary-heavy helper: a few operations per call, called once per sample from a JS
// loop. Every call into WASM would cost more than it computes, so it stays JS.

/** Smoothstep easing of `t` in `[0, 1]`. */
export function ease(t: number): number {
  const c = Math.min(Math.max(t, 0), 1);
  return c * c * (3 - 2 * c);
}
