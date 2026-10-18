// Mutating builtins throw on a frozen receiver, in sloppy code too, and freezing a non-empty
// typed array throws.
function attempt(label: string, run: () => unknown): void {
  try {
    run();
    console.log("ran", label);
  } catch (error) {
    console.log("threw", label, (error as Error).name);
  }
}

export function main(): void {
  const fa = Object.freeze([3, 1, 2]) as number[];
  try {
    fa.pop();
    console.log("popped");
  } catch (error) {
    console.log("threw pop", (error as Error).name);
  }
  try {
    fa.shift();
    console.log("shifted");
  } catch (error) {
    console.log("threw shift", (error as Error).name);
  }
  try {
    fa.reverse();
    console.log("reversed");
  } catch (error) {
    console.log("threw reverse", (error as Error).name);
  }
  try {
    fa.fill(0);
    console.log("filled");
  } catch (error) {
    console.log("threw fill", (error as Error).name);
  }
  try {
    fa.sort();
    console.log("sorted");
  } catch (error) {
    console.log("threw sort", (error as Error).name);
  }
  attempt("arrow pop", () => fa.pop());
  try {
    Object.freeze(new Uint8Array(2));
    console.log("froze bytes");
  } catch (error) {
    console.log("threw freezing bytes", (error as Error).name);
  }
  Object.freeze(new Uint8Array(0));
  const plain = [1, 2];
  plain.pop();
  console.log(plain.length, fa.length);
}
