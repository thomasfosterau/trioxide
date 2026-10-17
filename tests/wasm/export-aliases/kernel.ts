// Exported names that swap two functions, and an export the module later reassigns:
// importers must keep calling what the name refers to.
function sq(x: number): number {
  "use wasm";
  return x * x;
}

function cube(x: number): number {
  "use wasm";
  return x * x * x;
}

export { sq as cube, cube as sq };

export function tw(x: number): number {
  "use wasm";
  return x + 1;
}

export function swapTw(): void {
  // @ts-expect-error: valid JS, which importers see through the live binding.
  tw = (x: number): number => x + 1000;
}

export function localCalls(x: number): number {
  return sq(x) * 1000 + cube(x);
}
