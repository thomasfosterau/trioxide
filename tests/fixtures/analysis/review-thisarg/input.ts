function each(): unknown {
  let got: unknown = 0;
  [1].forEach(function (this: unknown) { got = this; }, { x: 1 });
  return got;
}
export function main(): void {
  console.log(each());
}
