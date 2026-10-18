import { ease } from "./ease.js";
import { tileDigest } from "./kernel.js";
import { render } from "./view.js";

const SEEDS = [7, 42, 1999];

/** The page's text: noise tile digests and an eased gradient. */
export function run(): string[] {
  const lines: string[] = [];
  for (const seed of SEEDS) {
    lines.push(`tile ${seed}: digest ${tileDigest(seed).toString(16)}`);
  }
  let gradient = 0;
  for (let i = 0; i < 100_000; i++) gradient += ease(i / 99_999);
  lines.push(`eased gradient: ${gradient.toFixed(6)}`);
  return lines;
}

export function main(): void {
  for (const line of run()) console.log(line);
}

if (typeof document !== "undefined") {
  const root = document.getElementById("app");
  if (root) render(root, run());
}
