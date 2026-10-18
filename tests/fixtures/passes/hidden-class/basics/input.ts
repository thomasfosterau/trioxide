// Positive cases: properties instances get late, or only sometimes, become fields declared up
// front, and `delete` of a property becomes an assignment of `undefined`.

class Particle {
  x: number;
  y: number;

  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
    // Only some particles get a boost: a second hidden class.
    if (x > 1) {
      this.boost = x;
    }
  }

  // Declared for TypeScript only: no field at run time.
  declare boost?: number;
}

class Entry {
  key: string;
  constructor(key: string) {
    this.key = key;
  }
}

function simulate(): number {
  let total = 0;
  for (let i = 0; i < 4; i++) {
    const p = new Particle(i, i * 2);
    total += p.x + p.y + (p.boost ?? 0);
  }
  return total;
}

function cache(): string {
  const entries: Entry[] = [];
  for (let i = 0; i < 3; i++) {
    const e = new Entry(`k${i}`) as Entry & { value?: number };
    e.value = i * 10;
    entries.push(e);
  }
  const first = entries[0] as Entry & { value?: number };
  // Deleting turns an object into a slow dictionary; assigning `undefined` doesn't.
  delete first.value;
  let out = "";
  for (const e of entries as (Entry & { value?: number })[]) out += `${e.key}=${e.value} `;
  return out;
}

export function main(): void {
  console.log(simulate(), cache());
}
