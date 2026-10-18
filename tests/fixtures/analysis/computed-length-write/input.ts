// A computed-key write whose key may be "length" (or an index) changes an array's length:
// the length the array was made with no longer holds after it.
export function main(): void {
  const xs = [1, 2, 3, 4];
  for (let i = 0; i < 1; i++) {
    const key = "len" + "gth";
    (xs as unknown as Record<string, number>)[key] = 2;
  }
  console.log(xs.length);
  const ys = [1, 2, 3];
  const index = ["1", "5"][1];
  (ys as unknown as Record<string, number>)[index] = 9;
  console.log(ys.length);
}
