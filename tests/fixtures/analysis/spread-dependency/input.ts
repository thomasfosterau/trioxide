// An object spread copies whatever the source holds by then, including properties another
// function adds after the copy is first analysed.

const source: { a: number; b?: number } = { a: 1 };

export function main(): void {
  const copy = { ...source };
  console.log(copy.a, copy.b /*@proven union*/);
}

function fill(): void {
  source.b = 2;
}

fill();
