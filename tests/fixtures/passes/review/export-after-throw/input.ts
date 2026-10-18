// An exported function after a top-level `throw` is still hoisted: a module of the cycle
// calls it while the throwing module is being linked and evaluated.
export async function main(): Promise<void> {
  try {
    await import("./thrower.js");
  } catch (error) {
    console.log("caught", (error as Error).message);
  }
}
