// Folding under a parenthesised callee must keep `this` unbound, an `eval` indirect, and a
// `typeof` of an undeclared name throwing when the original did.
class Counter {
  bump(): string {
    return this === undefined ? "unbound" : "bound";
  }
}
(globalThis as any).escaped = Counter;
const counter = new Counter();
const pick = Math.random() < 2;
const maybe: { bump(): string } | null = Math.random() < 2 ? counter : null;

function viaLiteral(): string {
  try {
    return (true && counter.bump)();
  } catch {
    return "threw";
  }
}

function viaConditional(): string {
  try {
    return (pick ? counter.bump : counter.bump)();
  } catch {
    return "threw";
  }
}

function viaCoalesce(): string {
  try {
    return (counter.bump ?? counter.bump)();
  } catch {
    return "threw";
  }
}

function viaChain(): string {
  try {
    return (maybe?.bump)!();
  } catch {
    return "threw";
  }
}

function viaEval(): string {
  const local = "local";
  // An indirect eval can't see `local`.
  return (true && eval)("typeof local");
}

function viaTypeof(): string {
  try {
    // @ts-ignore
    return typeof (true && notDeclaredAnywhere);
  } catch {
    return "threw";
  }
}

export function main(): void {
  console.log(viaLiteral(), viaConditional(), viaCoalesce(), viaChain(), viaEval(), viaTypeof());
}
