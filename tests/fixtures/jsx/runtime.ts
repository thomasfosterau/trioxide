export type Child = string | number | boolean | null | undefined | VNode | Child[];

export interface VNode {
  tag: string | typeof Fragment | ((props: Record<string, unknown>) => VNode);
  props: Record<string, unknown>;
  children: Child[];
}

export const Fragment = Symbol("Fragment");

export function h(tag: VNode["tag"], props: Record<string, unknown> | null, ...children: Child[]): VNode {
  return { tag, props: props ?? {}, children };
}

export function render(node: Child): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (Array.isArray(node)) return node.map(render).join("");
  if (typeof node !== "object") return String(node);
  if (node.tag === Fragment) return render(node.children);
  if (typeof node.tag === "function") return render(node.tag({ ...node.props, children: node.children }));
  const attrs = Object.entries(node.props)
    .map(([key, value]) => ` ${key}="${String(value)}"`)
    .join("");
  return `<${String(node.tag)}${attrs}>${render(node.children)}</${String(node.tag)}>`;
}
