import { bumpX, bumpY, move, move_p, shift, type P } from "./kernel.js";

function attempt(label: string, f: () => void): void {
  try {
    f();
    console.log(label, "ok");
  } catch (error) {
    console.log(label, (error as Error).constructor.name);
  }
}

export function main(): void {
  const a = { x: 1 };
  const b = { y: 1 };
  bumpX(a);
  bumpY(b);
  console.log(JSON.stringify(a), JSON.stringify(b));
  const k = { k: 1 };
  const j = { j: 1 };
  move_p(k);
  move(j);
  console.log(JSON.stringify(k), JSON.stringify(j));

  const frozen: P = Object.freeze({ x: 1, flag: false });
  attempt("frozen, no write", () => shift(frozen, 0));
  attempt("frozen, write", () => shift(frozen, 1));
  let reads = 0;
  const getter = {
    get x(): number {
      reads++;
      return 5;
    },
    flag: false,
  } as P;
  attempt("getter, no write", () => shift(getter, 0));
  attempt("getter, write", () => shift(getter, 1));
  console.log("getter reads", reads);
  const loose = { x: 1, flag: 1 } as unknown as P;
  shift(loose, 2);
  console.log(JSON.stringify(loose));
  const big = { x: 200, flag: false };
  shift(big, 1);
  console.log(JSON.stringify(big));
}
