// Guards and optional chains in a hot loop that the facts make redundant: `p` is always a
// particle and its fields always numbers. Benchmarked in the PR (`simulate`).

interface Particle {
  x: number;
  v: number;
  link?: Particle;
}

const DT = 0.5;
const COUNT = 64;

function particles(): Particle[] {
  const all: Particle[] = [];
  for (let i = 0; i < COUNT; i++) {
    all.push({ x: i, v: (i % 7) - 3 });
  }
  return all;
}

export function simulate(steps: number): number {
  const all = particles();
  let total = 0;
  for (let step = 0; step < steps; step++) {
    for (let i = 0; i < all.length; i++) {
      const p = all[i];
      if (p === undefined || p === null) {
        continue;
      }
      if (typeof p.x !== "number" || typeof p.v !== "number") {
        throw new TypeError("bad particle");
      }
      p.x += (p?.v ?? 0) * DT;
      total += p?.x ?? 0;
    }
  }
  return total;
}

export function main(): void {
  console.log(simulate(100));
}
