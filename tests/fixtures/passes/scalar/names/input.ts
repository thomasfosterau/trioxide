// Function names scalar replacement must keep. A local `p_f = () => 1` would name the
// function `p_f`, where the original's is empty (a member write doesn't name it) or its key.
const log: string[] = [];

export function main(): void {
  for (let i = 0; i < 2; i++) {
    // Not replaced: a later write gives a key an anonymous function.
    const p = { f: 0 as unknown as () => number, x: 2 };
    p.f = () => 1;
    log.push(`[${p.f.name}] ${p.x}`);
    // Not replaced: a logical assignment may give a key one.
    const o = { f: null as (() => number) | null, x: 3 };
    o.f ??= () => 2;
    log.push(`[${o.f.name}] ${o.x}`);
    // Not replaced: a key's initial value is one, behind a type assertion.
    const q = { f: (() => 0) as () => number, x: 3 };
    log.push(`[${q.f.name}] ${q.x}`);
    const r = { c: <new () => object>class {}, x: 4 };
    log.push(`[${r.c.name}] ${r.x}`);
    const s = { f: (() => 0)!, x: 5 };
    log.push(`[${s.f.name}] ${s.x}`);
    // Not replaced: an element is one (`[() => 0][0].name` is empty).
    const t: [() => number, number] = [() => 0, 6];
    log.push(`[${t[0].name}] ${t[1]}`);
    // Replaced: functions written to a key that are already named keep their names.
    function named(): number {
      return 7;
    }
    const u = { f: named, x: i };
    u.f = named;
    log.push(`[${u.f.name}] ${u.x}`);
  }
  console.log(log.join("\n"));
}
