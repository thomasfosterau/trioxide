// Calls partial evaluation must leave alone (or get exactly right).
let counter = 0;
const BEFORE = 2;

function bumped(): number {
  counter++;
  return counter;
}
function random(): number {
  return Math.random() < 2 ? 1 : 0;
}
function now(): number {
  return Date.now() > 0 ? 1 : 0;
}
function readsLet(): number {
  return counter + 1;
}
function throws(n: number): number {
  if (n > 0) throw new Error("no");
  return n;
}
function forever(): number {
  let i = 0;
  while (true) i++;
}
function shared(): number[][] {
  const row = [1, 2];
  return [row, row];
}
function big(): number[] {
  const out: number[] = [];
  for (let i = 0; i < 1000; i++) out.push(i);
  return out;
}
function usesLater(): number {
  return AFTER + 1;
}
function usesBefore(): number {
  return BEFORE * 21;
}
function strings(): string[] {
  return ["tab\there", "é", "quote\"s"];
}
function lone(): string {
  return String.fromCharCode(0xd800) + "x";
}
function loneLiteral(): string {
  return "\uD83D";
}
function invisible(): string {
  return "a\u200Bb";
}
function slow(): number {
  let s = 0;
  for (let i = 0; i < 1e6; i++) s = (s + i) % 7;
  return s;
}
function loopTdz(): number {
  let s = 0;
  for (let i = 0; i < 3; i++) {
    if (i > 0) s += y;
    const y = i;
  }
  return s;
}
function charCode(): number {
  return "A".charCodeAt(NaN);
}
function holes(): number[] {
  return new Array(3);
}
function transcendental(): number {
  return Math.sin(1);
}
function exact(): number[] {
  return [Math.sqrt(2), Math.fround(0.1), 2 ** 52 + 1, -(2 ** 31) | 0, 7 % -3, -7 % 3, 1 / -0];
}

const C1 = bumped();
const C2 = random();
const C3 = now();
const C4 = readsLet();
let C5 = 0;
try {
  C5 = throws(1);
} catch {
  C5 = -1;
}
const C6 = shared();
const C7 = big().length;
let C8 = 0;
try {
  C8 = usesLater();
} catch {
  C8 = -1;
}
const AFTER = 5;
const C9 = usesBefore();
const C10 = strings();
const C11 = holes();
const C12 = transcendental();
const C13 = exact();
const C14 = [lone(), loneLiteral(), invisible()];
const C15 = slow();
let C16 = "";
try {
  C16 = String(loopTdz());
} catch (e) {
  C16 = (e as Error).name;
}
const C18 = charCode();

export function main(): void {
  C6[0][0] = 9;
  console.log(C1, C2, C3, C4, C5, C6[1][0], C7, C8, C9);
  console.log(C10.map((s) => s.length).join(","), C11.length, 0 in C11, C12 > 0.84);
  console.log(C13.map((x) => (Object.is(x, -0) ? "-0" : String(x))).join(","));
  console.log(C16, C18);
  console.log(C14.map((s) => s.length + ":" + s.charCodeAt(0)).join(","), C15, typeof forever);
}
