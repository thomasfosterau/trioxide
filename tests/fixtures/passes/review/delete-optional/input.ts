// Review: a `delete` is never pure, so it is never removed, and `checks` may drop its `?.`
// only when the object is proven non-nullish (so the delete still happens).

function optional(): number | undefined {
  const p: { late?: number } | undefined = { late: 2 };
  delete p?.late;
  return p.late;
}

function plain(): number | undefined {
  const q: { late?: number } = { late: 3 };
  delete q.late;
  return q.late;
}

function nullish(): string {
  const r: { late?: number } | undefined = Math.random() > 2 ? { late: 4 } : undefined;
  delete r?.late;
  return String(r);
}

function result(): boolean {
  const s: { late?: number } | null = { late: 5 };
  const removed = delete s?.late;
  return removed && !("late" in s);
}

function wrapped(): string {
  const t: { a?: number; b?: number } = { a: 6, b: 7 };
  // A parenthesised or type-asserted operand is still the property.
  delete (t.a);
  delete (t as { b?: number }).b;
  return `${t.a} ${t.b}`;
}

function nested(): number | undefined {
  const u: { inner?: { late?: number } } = { inner: { late: 8 } };
  delete u.inner?.late;
  return u.inner?.late;
}

export function main(): void {
  console.log(optional(), plain(), nullish(), result(), wrapped(), nested());
}
