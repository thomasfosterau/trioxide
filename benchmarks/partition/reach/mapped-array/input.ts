import { mean } from "./kernel.js";

const base = [1, 2, 3, 4, 5, 6, 7, 8];

export function main(): void {
  console.log(mean(base.map((v) => v * 1.5)));
}
