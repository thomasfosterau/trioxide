// `__proto__` is the prototype, not an own key: the call stays (reaching a prototype this way
// also turns off the analysis's builtin models, and the interpreter refuses the key itself).
function proto(): string {
  const o = { a: 1 };
  return typeof (o as { __proto__?: unknown }).__proto__;
}

const P = proto();

export function main(): void {
  console.log(P);
}
