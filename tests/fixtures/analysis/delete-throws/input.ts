// In strict code, deleting a non-configurable property throws: an array's `length`, a typed
// array's elements, any property of a frozen object (writing one throws too).
export function main(): void {
  const xs = [1, 2, 3];
  for (const k of ["0", "length"]) {
    try {
      delete (xs as unknown as Record<string, number>)[k];
      console.log("deleted", k);
    } catch (error) {
      console.log("threw", k, (error as Error).name);
    }
  }
  const frozen = Object.freeze({ a: 1 });
  try {
    delete (frozen as { a?: number }).a;
    console.log("deleted a");
  } catch (error) {
    console.log("threw a", (error as Error).name);
  }
  try {
    (frozen as { a: number }).a = 2;
    console.log("wrote a");
  } catch (error) {
    console.log("threw writing a", (error as Error).name);
  }
  const bytes = new Uint8Array(2);
  for (const k of ["1", "length"]) {
    try {
      delete (bytes as unknown as Record<string, number>)[k];
      console.log("deleted byte", k);
    } catch (error) {
      console.log("threw byte", k, (error as Error).name);
    }
  }
}
