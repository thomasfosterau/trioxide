// A number[] must not change length: a write the analysis can't prove in bounds could grow it.
function append(values: number[], value: number): void {
  "use wasm";
  values[values.length] = value;
}

export function main(): void {
  const values = [1, 2];
  append(values, 3);
  console.log(values);
}
