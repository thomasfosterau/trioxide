// DOM code: stays JS (islands can't touch host APIs or strings).

/** Renders `lines` as a list under `root`. */
export function render(root: HTMLElement, lines: string[]): void {
  const list = document.createElement("ul");
  for (const line of lines) {
    const item = document.createElement("li");
    item.textContent = line;
    list.append(item);
  }
  root.replaceChildren(list);
}
