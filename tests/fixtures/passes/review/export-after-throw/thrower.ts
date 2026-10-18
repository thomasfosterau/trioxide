import "./user.js";
throw new Error("thrower failed");
// Not constant, so the calls stay calls.
export function hoisted(): number {
  return Date.now() > 0 ? 1 : 0;
}
export default function hoistedDefault(): number {
  return Date.now() > 0 ? 2 : 0;
}
