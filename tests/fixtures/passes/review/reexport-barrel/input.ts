// A named re-export links to its module's export even when nothing imports it.
import { used } from "./hub.js";
export function main(): void {
  console.log(used);
}
