// Effect summaries: what calling a function may do, including its callees.

let state = 0;

function pure(a: number, b: number): number {
  return Math.max(a, b) * 2;
}

function reader(): number {
  return state;
}

function writer(value: number): void {
  state = value;
}

function thrower(value: number): number {
  if (value < 0) {
    throw new RangeError("negative");
  }
  return Math.sqrt(value);
}

function allocator(value: number): { value: number } {
  return { value };
}

function caller(): number {
  writer(pure(1, 2));
  return reader();
}

function catches(): number {
  try {
    return thrower(-1);
  } catch {
    return 0;
  }
}

export function main(): void {
  console.log(pure(3, 4), caller(), catches(), allocator(1).value, thrower(4));
}
