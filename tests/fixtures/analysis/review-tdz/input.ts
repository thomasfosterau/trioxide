function early(): number {
  return late;
}
let result = "none";
try {
  early();
  result = "no throw";
} catch {
  result = "threw";
}
const late = 1;

export function main(): void {
  console.log(result, early());
}
