// A module namespace that escapes (here through a dynamic import) before the module's own
// body has run: the functions it exports are hoisted later in the analysis, and unknown code
// can still reach and call them through it.
(globalThis as any)["r" + "un"] = (lib: any): unknown => lib.twice("from unknown code");

export async function main(): Promise<void> {
  const lib = await import("./lib.js");
  const run = (globalThis as any)["r" + "un"];
  console.log(run(lib), lib.two);
}
