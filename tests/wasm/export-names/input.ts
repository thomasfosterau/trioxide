import { __trioxide_alloc, memory, x$1, x_1 } from "./kernel.js";

export function main(): void {
  console.log(memory(1), __trioxide_alloc(2), x$1(3), x_1(4));
}
