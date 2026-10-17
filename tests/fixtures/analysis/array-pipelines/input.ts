// map/filter/reduce over proven arrays: callbacks get proven elements and indices.

export function main(): void {
  const values = [1, 2, 3, 4];
  const doubled = values.map((v) => v /*@proven number int*/ * 2);
  const even = doubled.filter((v, i) => i /*@proven number int nonNaN*/ % 2 === 0);
  const total = even.reduce((acc, v) => acc + v, 0);
  console.log(total /*@proven number*/);
  // (Passing the arrays themselves to console.log would hand them to unknown code.)
  console.log((doubled /*@proven array*/).length, (even /*@proven array*/).length);
  const words = "a b c".split(" ");
  console.log(words.join("-") /*@proven string*/, (words /*@proven array*/).length);
  const found = values.find((v) => v > 2);
  console.log(found /*@proven union*/);
  values.forEach((v, i, all) => {
    console.log(v + i, (all /*@proven array*/).length);
  });
}
