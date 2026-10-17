// Destructuring reads properties and iterates; defaults replace undefined.

function options({ size = 1, name }: { size?: number; name: string }): string {
  return (name /*@proven string*/) + (size /*@proven number int*/);
}

export function main(): void {
  const [first, second = 10, ...rest] = [1, 2, 3, 4];
  console.log(first /*@proven union*/, second /*@proven number int*/, (rest /*@proven array*/).length);
  const { x, y: why, ...others } = { x: 1, y: "two", z: true };
  console.log(x /*@proven number int*/, why /*@proven string*/, others /*@proven object*/);
  console.log(options({ name: "a" }), options({ name: "b", size: 2 }));
  let a = 1;
  let b = 2;
  [a, b] = [b, a];
  console.log(a /*@proven union*/, b /*@proven union*/);
}
