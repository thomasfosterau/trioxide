import { a } from "./input.js";
let seen = "unset";
try {
  seen = String(a);
} catch {
  seen = "tdz";
}
export function readA(): string {
  return seen;
}
