// Primitive strings: nothing can mutate them, so their methods are pure.

export function main(): void {
  const text = "Hello, World";
  const upper = text.toUpperCase();
  console.log(upper /*@proven string*/, text.length /*@proven number int nonNaN*/);
  const index = text.indexOf("o");
  console.log(index /*@proven number int*/, text.charCodeAt(0) /*@proven number*/);
  const parts = text.split(", ");
  console.log(parts.length /*@proven number int*/, text[0] /*@proven union*/);
  const joined = `${text}!` + 1;
  console.log(joined /*@proven string*/);
  const n = 7;
  console.log(n.toFixed(2) /*@proven string*/);
}
