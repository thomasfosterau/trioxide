// Recursive structures: types are cut off at a fixed depth instead of growing forever.

type Tree = { value: number; children: Tree[] };

function make(depth: number): Tree {
  const children: Tree[] = [];
  if (depth > 0) {
    children.push(make(depth - 1), make(depth - 1));
  }
  return { value: depth, children };
}

function count(tree: Tree): number {
  let total = 1;
  for (const child of tree.children) {
    total += count(child);
  }
  return total;
}

export function main(): void {
  const tree = make(3);
  console.log(count(tree) /*@proven number*/, tree.value /*@proven number int*/);
}
