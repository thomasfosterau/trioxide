// Function names inlining must keep: an anonymous function argument moved into a binding or
// an assignment would take that binding's name. Also where temporaries are declared.
function id(cb: () => number): () => number {
  return cb;
}

function same<T>(value: T): T {
  return value;
}

let n = 0;
function next(): number {
  return ++n;
}

function add(a: number, b: number): number {
  return a + (b = b + 1, b);
}

function len(x: number, y: number): number {
  return x * x + y * y;
}

function names(): string {
  const g = id(() => 1);
  let i;
  i = id(() => 2);
  const k = same(class {});
  const typed = id((() => 3) as () => number);
  return `[${g.name}] [${i.name}] [${k.name}] [${typed.name}] ${g() + i() + typed()}`;
}

function temporaries(): number {
  // Arguments evaluated first by the statement are declared with their values; others are
  // assigned inside the expression, with their `let` just before the statement.
  const r = add(next(), next());
  console.log(add(next(), next()), r);
  return len(next(), next() + 1);
}

export function main(): void {
  console.log(names());
  console.log(temporaries());
}
