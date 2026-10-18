// trustTypes changes nothing for scalar replacement (its facts are never only Declared).
export function report(p: { x: number; y: number }): number {
  return p.x + p.y;
}

export function main(): void {
  const p = { x: 1, y: 2 };
  const q = { x: 3, y: 4 };
  console.log(report(p), q.x * q.y);
}
