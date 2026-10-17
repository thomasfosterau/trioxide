// The n-body simulation from the Computer Language Benchmarks Game, over one flat
// Float64Array holding [x, y, z, vx, vy, vz, mass] per body.

const STRIDE = 7;

/** The velocity change per unit mass for bodies (dx, dy, dz) apart over dt. */
function pull(dx: number, dy: number, dz: number, dt: number): number {
  const d2 = dx * dx + dy * dy + dz * dz;
  return dt / (d2 * Math.sqrt(d2));
}

export function advance(bodies: Float64Array, dt: number, steps: number): void {
  const n = bodies.length / STRIDE;
  for (let step = 0; step < steps; step++) {
    for (let i = 0; i < n; i++) {
      const bi = i * STRIDE;
      for (let j = i + 1; j < n; j++) {
        const bj = j * STRIDE;
        const dx = bodies[bi] - bodies[bj];
        const dy = bodies[bi + 1] - bodies[bj + 1];
        const dz = bodies[bi + 2] - bodies[bj + 2];
        const mag = pull(dx, dy, dz, dt);
        const mi = bodies[bi + 6] * mag;
        const mj = bodies[bj + 6] * mag;
        bodies[bi + 3] -= dx * mj;
        bodies[bi + 4] -= dy * mj;
        bodies[bi + 5] -= dz * mj;
        bodies[bj + 3] += dx * mi;
        bodies[bj + 4] += dy * mi;
        bodies[bj + 5] += dz * mi;
      }
    }
    for (let i = 0; i < n; i++) {
      const b = i * STRIDE;
      bodies[b] += dt * bodies[b + 3];
      bodies[b + 1] += dt * bodies[b + 4];
      bodies[b + 2] += dt * bodies[b + 5];
    }
  }
}

export function energy(bodies: Float64Array): number {
  const n = bodies.length / STRIDE;
  let e = 0;
  for (let i = 0; i < n; i++) {
    const bi = i * STRIDE;
    const m = bodies[bi + 6];
    e += 0.5 * m * (bodies[bi + 3] ** 2 + bodies[bi + 4] ** 2 + bodies[bi + 5] ** 2);
    for (let j = i + 1; j < n; j++) {
      const bj = j * STRIDE;
      e -= (m * bodies[bj + 6]) / distance(bodies[bi] - bodies[bj], bodies[bi + 1] - bodies[bj + 1], bodies[bi + 2] - bodies[bj + 2]);
    }
  }
  return e;
}

export function distance(dx: number, dy: number, dz: number): number {
  return Math.sqrt(dx * dx + dy * dy + dz * dz);
}
