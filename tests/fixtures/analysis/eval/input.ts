// A direct eval can read and write every binding it can see; an indirect one only globals.

export function direct(code: string): number {
  let x = 1;
  eval(code);
  // eval may have assigned x.
  return x /*@unknown*/;
}

export function indirect(code: string): number {
  let y = 1;
  (0, eval)(code);
  // An indirect eval can't see y.
  return y /*@proven number int*/;
}

export function main(): void {
  console.log(direct("x = 'changed'"), indirect("1"));
}
