// Adversarial cases for check elimination.

function castAway(): unknown {
  // A cast doesn't make a value non-null: the analysis knows it may be null.
  const value = (Math.random() > 2 ? { x: 1 } : null) as { x: number };
  return value?.x;
}

function shadowed(undefined: number): boolean {
  // `undefined` is a parameter here, so this compares with a number.
  const n = 1;
  return n === undefined;
}

function getterChain(): unknown {
  let reads = 0;
  const box = {
    get inner(): { y: number } | undefined {
      reads++;
      return reads > 1 ? { y: 2 } : undefined;
    },
  };
  // The getter runs on each read and may return undefined.
  return [box.inner?.y, box.inner?.y, reads];
}

function typeofEffects(): unknown {
  let calls = 0;
  const f = (): number => {
    calls++;
    return 1;
  };
  // The operand has effects, so the comparison stays.
  return [typeof f() === "number", calls];
}

function unknownValue(input: unknown): string {
  return typeof input === "string" ? "string" : "other";
}

export function main(): void {
  console.log(castAway(), shadowed(1), shadowed(2), getterChain(), typeofEffects());
  console.log(unknownValue("a"), unknownValue(JSON.parse("1")));
}
