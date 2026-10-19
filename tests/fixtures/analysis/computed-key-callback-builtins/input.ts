// Functions held by objects and read back where the analysis loses track of the slot:
// builtins that return an object's values, `for…in`, spread copies and computed reads.
function v1(x: number): number {
  return x * 2;
}
function v2(x: number): number {
  return x + 1;
}
function v3(x: number): number {
  return x - 1;
}
function v4(x: number): number {
  return x * 3;
}
function v5(x: number): number {
  return x * 5;
}
function v6(x: number): number {
  return x * 7;
}
const byValues = { f: v1 };
const byLoop = { f: v2 };
const bySpread = { f: v3 };
const byIndex = [v4];
const byName = { f: v5 };
const byEntries = { f: v6 };
export function main(): void {
  console.log(Object.values(byValues)[0]!(1));
  for (const key in byLoop) {
    console.log((byLoop as any)[key](2));
  }
  const copy = { ...bySpread };
  console.log(copy.f(3));
  const i = Number("0");
  console.log(byIndex[i]!(4));
  const k = ["f"][0]!;
  console.log((byName as any)[k](5));
  console.log(Object.entries(byEntries)[0]![1](6));
}
