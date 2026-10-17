// Hoisted `var`s are `undefined` before their assignment; functions are callable early.

export function main(): void {
  console.log(early() /*@proven string*/);
  console.log(later /*@proven undefined*/);
  var later = 5;
  console.log(later /*@proven number int*/);
  function early(): string {
    return "early";
  }
  let count = 0;
  const bump = (): void => {
    count += 1;
  };
  bump();
  console.log(count /*@proven number int*/);
}
