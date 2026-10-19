export interface Row {
  key: number;
  amount: number;
}

export function makeRows(count: number): Row[] {
  const rows: Row[] = [];
  let state = 7;
  for (let i = 0; i < count; i++) {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    rows.push({ key: Math.floor((state / 4294967296) * 8), amount: (state % 100) / 10 });
  }
  return rows;
}
