import { ease } from "./ease.js";
import { escapeTotal } from "./kernel.js";
import { render } from "./view.js";

const VIEWS = [
  { name: "whole set", x0: -2.5, y0: -1.25, x1: 1, y1: 1.25 },
  { name: "seahorse valley", x0: -0.8, y0: 0, x1: -0.7, y1: 0.1 },
];

/** The page's text: escape statistics per view and an eased gradient. */
export function run(): string[] {
  const lines: string[] = [];
  for (const view of VIEWS) {
    const total = escapeTotal(view.x0, view.y0, view.x1, view.y1, 240, 160, 300);
    lines.push(`${view.name}: ${total} iterations`);
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
