import { cube, localCalls, sq, swapTw, tw } from "./kernel.js";

export function main(): void {
  console.log(sq(3), cube(3), localCalls(2), tw(1));
  swapTw();
  console.log(tw(1));
}
