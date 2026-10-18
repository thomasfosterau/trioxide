// Objects scalar replacement must leave alone, each for one reason, next to one it replaces.
const seen = new WeakSet<object>();
const log: string[] = [];

export function main(): void {
  for (let i = 0; i < 2; i++) {
    // Replaced: only its keys are read and written.
    const ok = { a: i, b: 2 };
    ok.a *= 3;

    const compared = { a: i };
    const same = compared === compared;
    const inKey = { a: i };
    const has = "a" in inKey;
    const deleted = { a: i, b: 1 };
    delete (deleted as { b?: number }).b;
    const spread = { a: i };
    const copy = { ...spread };
    const json = { a: i };
    const text = JSON.stringify(json);
    const keys = { a: i, b: 2 };
    const names = Object.keys(keys).join("");
    const iterated = { a: i };
    let forIn = "";
    for (const key in iterated) forIn += key;
    const passed = { a: i };
    seen.add(passed);
    const method = { a: i, get(): number { return this.a; } };
    const accessor = { get a(): number { return i; } };
    const proto = { __proto__: { a: 1 }, b: i } as { a: number; b: number };
    const computed = { ["a" + ""]: i };
    const missing = { a: i } as { a: number; b?: number };
    const tuple = [i, i + 1];
    const reassigned = { a: i };
    let alias = reassigned;
    alias.a = 9;
    const optional = { a: i } as { a: number } | undefined;
    const viaClosure = { a: i };
    const read = (): number => viaClosure.a;
    log.push(
      [
        ok.a + ok.b,
        same,
        has,
        deleted.b ?? "gone",
        copy.a,
        text,
        names,
        forIn,
        seen.has(passed),
        method.get(),
        accessor.a,
        proto.a + proto.b,
        computed.a,
        missing.b,
        tuple[0] + tuple[1],
        reassigned.a,
        optional?.a,
        read(),
      ].join(" "),
    );
  }
  console.log(log.join("\n"));
}
