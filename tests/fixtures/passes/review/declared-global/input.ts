// Only TypeScript says a `declare`d binding exists: reading it may throw.
declare let missing: number;
declare const __DEV__: boolean;
export function main(): void {
  let result = "none";
  try {
    const m = missing;
  } catch {
    result = "threw";
  }
  try {
    missing;
  } catch {
    result += " again";
  }
  try {
    if (__DEV__) {
      result += " dev";
    }
  } catch {
    result += " no define";
  }
  console.log(result);
}
