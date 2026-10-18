import { tileDigest } from "./kernel.js";

export const run = (): number => {
  return tileDigest(7) ^ tileDigest(42) ^ tileDigest(1999);
};

export function main(): void {
  console.log(run());
}
