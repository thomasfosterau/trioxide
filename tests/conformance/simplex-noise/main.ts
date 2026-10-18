// Conformance driver (not part of simplex-noise): samples every noise function with a
// seeded generator; the harness compares the output across builds.
import { buildPermutationTable, createNoise2D, createNoise3D, createNoise4D } from "./simplex-noise.js";

function lcg(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 4294967296;
  };
}

export function main(): void {
  const table = buildPermutationTable(lcg(1));
  console.log("table", table.length, Array.from(table.subarray(0, 16)).join(","));
  const noise2D = createNoise2D(lcg(2));
  const noise3D = createNoise3D(lcg(3));
  const noise4D = createNoise4D(lcg(4));
  let sum2 = 0;
  let sum3 = 0;
  let sum4 = 0;
  const samples: string[] = [];
  for (let y = 0; y < 64; y++) {
    for (let x = 0; x < 64; x++) {
      const a = noise2D(x * 0.07, y * 0.05);
      const b = noise3D(x * 0.03, y * 0.04, (x + y) * 0.01);
      const c = noise4D(x * 0.02, y * 0.03, x * 0.01, y * 0.015);
      sum2 += a;
      sum3 += b;
      sum4 += c;
      if ((x * 64 + y) % 509 === 0) samples.push(`${a}/${b}/${c}`);
    }
  }
  console.log("sums", sum2, sum3, sum4);
  console.log(samples.join("\n"));
  console.log("edges", noise2D(0, 0), noise3D(1e9, -1e9, 0.5), noise4D(-0, 0, 0, 0), Number.isNaN(noise2D(NaN, 1)));
}
