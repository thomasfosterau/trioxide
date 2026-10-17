export function compareAssign(): string {
  let x: number | string = 1;
  if (x === ((x = "a"), 1)) {
    const seen = x;
    return String(seen);
  }
  return "no";
}

export function main(): void {
  console.log(compareAssign());
}
