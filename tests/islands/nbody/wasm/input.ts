import { advance, distance, energy } from "./kernel.js";

const PI = Math.PI;
const SOLAR_MASS = 4 * PI * PI;
const DAYS_PER_YEAR = 365.24;

// Sun, Jupiter, Saturn, Uranus, Neptune.
const SYSTEM = [
  [0, 0, 0, 0, 0, 0, SOLAR_MASS],
  [4.84143144246472090e00, -1.16032004402742839e00, -1.03622044471123109e-01, 1.66007664274403694e-03 * DAYS_PER_YEAR, 7.69901118419740425e-03 * DAYS_PER_YEAR, -6.90460016972063023e-05 * DAYS_PER_YEAR, 9.54791938424326609e-04 * SOLAR_MASS],
  [8.34336671824457987e00, 4.12479856412430479e00, -4.03523417114321381e-01, -2.76742510726862411e-03 * DAYS_PER_YEAR, 4.99852801234917238e-03 * DAYS_PER_YEAR, 2.30417297573763929e-05 * DAYS_PER_YEAR, 2.85885980666130812e-04 * SOLAR_MASS],
  [1.28943695621391310e01, -1.51111514016986312e01, -2.23307578892655734e-01, 2.96460137564761618e-03 * DAYS_PER_YEAR, 2.37847173959480950e-03 * DAYS_PER_YEAR, -2.96589568540237556e-05 * DAYS_PER_YEAR, 4.36624404335156298e-05 * SOLAR_MASS],
  [1.53796971148509165e01, -2.59193146099879641e01, 1.79258772950371181e-01, 2.68067772490389322e-03 * DAYS_PER_YEAR, 1.62824170038242295e-03 * DAYS_PER_YEAR, -9.51592254519715870e-05 * DAYS_PER_YEAR, 5.15138902046611451e-05 * SOLAR_MASS],
];

function system(): Float64Array {
  const bodies = new Float64Array(SYSTEM.flat());
  // Offset the sun's momentum so the system's centre of mass stays still.
  let px = 0;
  let py = 0;
  let pz = 0;
  for (let i = 0; i < bodies.length; i += 7) {
    px += bodies[i + 3] * bodies[i + 6];
    py += bodies[i + 4] * bodies[i + 6];
    pz += bodies[i + 5] * bodies[i + 6];
  }
  bodies[3] = -px / SOLAR_MASS;
  bodies[4] = -py / SOLAR_MASS;
  bodies[5] = -pz / SOLAR_MASS;
  return bodies;
}

export function main(): void {
  const bodies = system();
  console.log(energy(bodies).toFixed(9), energy(bodies));
  advance(bodies, 0.01, 1000);
  console.log(energy(bodies).toFixed(9), energy(bodies));
  console.log(Array.from(bodies.subarray(7, 14)));

  // A length that isn't a multiple of 7: the last partial body reads past the end, which
  // JS turns into NaN, and its writes past the end are dropped.
  const ragged = system().subarray(0, 17);
  advance(ragged, 0.01, 3);
  console.log(Array.from(ragged), energy(ragged));
  console.log(energy(new Float64Array(0)), energy(new Float64Array(7)));

  for (const [dx, dy, dz] of [[3, 4, 12], [-0, 0, -0], [NaN, 1, 1], [Infinity, NaN, 0], [1e200, 1e200, 0], [5e-324, 0, 0]]) {
    console.log(dx, dy, dz, distance(dx, dy, dz));
  }
}
