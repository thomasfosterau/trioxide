// Only plain objects can cross the boundary; class instances need their prototype.
class Counter {
  count = 0;
}

function total(counter: Counter): number {
  "use wasm";
  return counter.count;
}

export function main(): void {
  console.log(total(new Counter()));
}
