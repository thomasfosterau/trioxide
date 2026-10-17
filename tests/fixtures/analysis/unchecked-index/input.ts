// Index reads past the end give undefined, whatever TypeScript's element type says.

function at(values: number[], index: number): number {
  return values[index];
}

export function main(): void {
  const values = [1, 2, 3];
  const past = values[10];
  console.log(past /*@proven union*/);
  const tuple: [number, string] = [1, "a"];
  console.log(tuple[1] /*@proven union*/);
  console.log(at(values, 5) /*@proven union*/);
}
