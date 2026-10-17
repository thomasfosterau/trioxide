import { filled, total } from "./kernel.js";

function attempt(label: string, f: () => unknown): void {
  try {
    const result = f();
    console.log(label, "ok", result instanceof Float64Array ? Array.from(result) : result);
  } catch (error) {
    console.log(label, "threw", (error as Error).constructor.name);
  }
}

export function main(): void {
  attempt("filled(3, 1.5)", () => filled(3, 1.5));
  attempt("filled(2.9, 1)", () => filled(2.9, 1));
  attempt("filled(NaN, 1)", () => filled(NaN, 1));
  attempt("filled(-0.5, 1)", () => filled(-0.5, 1));
  attempt("filled(-1, 1)", () => filled(-1, 1));
  attempt("filled(-Infinity, 1)", () => filled(-Infinity, 1));
  attempt("filled(2 ** 53, 1)", () => filled(2 ** 53, 1));
  attempt("total(2 ** 40)", () => total(2 ** 40));
  attempt("total(1e300)", () => total(1e300));
  // Still usable after the traps above.
  attempt("total(100)", () => total(100));
  attempt("filled(4, -0)", () => filled(4, -0));
}
