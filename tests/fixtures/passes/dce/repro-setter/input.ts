// S2 repros: object-literal setters, and protocol methods (`Symbol.toPrimitive`), run user
// code. The call to viaSetter and the method must not be removed.
let log = 0;
function viaSetter(): number { const s = { set p(n: number) { log += n; } }; s.p = 3; return 0; }
const o = { [Symbol.toPrimitive](hint: string): number { log += hint.length; return 1; } };
export function main(): void { viaSetter(); console.log(+(o as any), log); }
