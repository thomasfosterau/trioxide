import { loaded } from "./sibling.js";
import { scale } from "./kernel.js";

console.log("input body", loaded, scale(4, 5));

export function main(): void {
  console.log("main", scale(6, 7));
}
