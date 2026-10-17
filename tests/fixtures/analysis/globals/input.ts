// Globals are outside the program: their values are unknown, and writes to them escape.

declare global {
  var appState: { ready: boolean } | undefined;
}

export function main(): void {
  globalThis.appState = { ready: true };
  console.log(globalThis.appState?.ready /*@declared boolean*/);
  console.log(typeof setTimeout /*@proven string*/, undefined /*@proven undefined*/, NaN /*@proven number*/);
}
