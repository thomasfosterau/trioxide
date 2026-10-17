// Captured variables: a closure that writes a local makes it flow-insensitive.

export function counter(): () => number {
  let count = 0;
  return () => {
    count++;
    return count /*@proven number int*/;
  };
}

export function sharedWrite(): number {
  let value: number | string = 1;
  const set = (): void => {
    value = "set by closure";
  };
  set();
  // Any call could have run `set`: the value is either.
  return (value /*@proven union*/) as number;
}

export function readOnlyCapture(): number {
  const base = 10;
  const add = (x: number): number => (x + base /*@proven number int*/);
  return add(5) /*@proven number int*/;
}

export function main(): void {
  const next = counter();
  next();
  console.log(next(), sharedWrite(), readOnlyCapture());
}
