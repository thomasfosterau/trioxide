// The timing harness every benchmark uses (docs/PLAN.md §8): each variant of a program runs
// in processes of its own (worker.mjs), over several rounds whose order alternates (round r
// starts at variant r mod n, and odd rounds go backwards), under each engine configuration.
// It reports, per variant, the median time per call and, against the first variant, the
// median and range of the per-round speedups.

import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

/** V8's defaults, and cold code: interpreter and baseline compiler only (no Maglev, no TurboFan). */
export const ENGINES = [
  { name: 'default', flags: [] },
  { name: 'cold', flags: ['--no-opt', '--no-maglev'] },
];

export function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = sorted.length >> 1;
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

/** Runs the worker once: `{ ns, check }`. */
export function timeOnce(flags, variant, budget) {
  const args = [...flags, path.join(here, 'worker.mjs'), variant.entry, variant.export ?? 'run', variant.mode ?? 'run', String(budget), ...(variant.glues ?? [])];
  const output = execFileSync(process.execPath, args, {
    encoding: 'utf8',
    cwd: path.dirname(variant.entry),
    maxBuffer: 64 * 1024 * 1024,
  });
  return JSON.parse(output);
}

/** The order of the variants in round `round`. */
export function order(count, round) {
  const indices = Array.from({ length: count }, (_, i) => (i + round) % count);
  return round % 2 ? indices.reverse() : indices;
}

/**
 * Times `variants` (`{ name, entry, export?, mode?, glues? }`; the first is the baseline)
 * under every engine. Returns one row per engine: `{ engine, variants: [{ name, ns, speedup,
 * low, high }] }`, where `ns` is the median of the per-round times and the speedups are
 * baseline time / variant time per round.
 */
export function compare(variants, { engines = ENGINES, rounds = 5, budget = 1000, label = '' } = {}) {
  const rows = [];
  for (const engine of engines) {
    const times = variants.map(() => []);
    for (let round = 0; round < rounds; round++) {
      const checks = new Map();
      for (const index of order(variants.length, round)) {
        const result = timeOnce(engine.flags, variants[index], budget);
        times[index].push(result.ns);
        checks.set(index, result.check);
      }
      for (const [index, check] of checks) {
        assert.equal(check, checks.get(0), `${label}: ${variants[index].name} computes something else than ${variants[0].name}`);
      }
    }
    rows.push({
      engine: engine.name,
      variants: variants.map((variant, index) => {
        const ratios = times[index].map((ns, round) => times[0][round] / ns);
        return {
          name: variant.name,
          ns: median(times[index]),
          speedup: median(ratios),
          low: Math.min(...ratios),
          high: Math.max(...ratios),
        };
      }),
    });
  }
  return rows;
}

export function formatTime(ns) {
  if (ns == null) return '—';
  if (ns >= 1e9) return `${(ns / 1e9).toFixed(2)} s`;
  if (ns >= 1e6) return `${(ns / 1e6).toFixed(2)} ms`;
  if (ns >= 1e3) return `${(ns / 1e3).toFixed(1)} µs`;
  return `${ns.toFixed(0)} ns`;
}

export function formatSpeedup(v) {
  return `${v.speedup.toFixed(2)}× (${v.low.toFixed(2)}–${v.high.toFixed(2)})`;
}

/** A markdown table from a header row and rows of cells. */
export function table(header, rows) {
  return [
    `| ${header.join(' | ')} |`,
    `|${header.map((_, i) => (i === 0 ? '---' : '---:')).join('|')}|`,
    ...rows.map((row) => `| ${row.join(' | ')} |`),
  ].join('\n');
}
