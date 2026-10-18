// Runtime benchmarks of trioxide's JS passes (docs/PLAN.md §8): every case in cases/ is
// compiled twice, types stripped only (`trioxide strip`) and optimised (`trioxide build
// --emit=js --no-wasm`, with the case's trioxide.json, which turns on the passes that are off
// by default), and each version's `bench()` is timed with tinybench in a process of its own
// (worker.mjs), so one version's JIT state can't skew the other's. Each engine configuration
// runs ROUNDS rounds, alternating which version goes first; the table reports the median
// throughput of each version and the median and range of the per-round speedups.
//
// Engine configurations: V8's defaults, whose optimising tiers already inline small functions
// and replace non-escaping objects in hot loops, and `--no-opt --no-maglev` (interpreter and
// baseline compiler only), which is how code runs before it is hot, or when it never gets hot.
//
// A case that exports `measure = "init"` is timed by importing the module afresh (a new URL
// each time), which is where compile-time partial evaluation pays off. That time includes
// reading and parsing the file, so the optimised version's bigger literal counts against it.
//
// Set TRIOXIDE_BIN to use a prebuilt binary; otherwise `cargo build -p trioxide_cli` runs
// first. CASE filters cases by substring; ROUNDS (default 5) and BENCH_TIME (milliseconds per
// sample, default 500) trade time for precision.

import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const out = path.join(here, '.out');
const time = process.env.BENCH_TIME ?? '500';
const rounds = Number(process.env.ROUNDS ?? 5);
const engines = [
  { name: 'default', flags: [] },
  { name: 'cold', flags: ['--no-opt', '--no-maglev'] },
];

let trioxide = process.env.TRIOXIDE_BIN;
if (!trioxide) {
  execFileSync('cargo', ['build', '--quiet', '-p', 'trioxide_cli'], { cwd: repo, stdio: 'inherit' });
  trioxide = path.join(repo, 'target', 'debug', 'trioxide');
}

/** Compiles a case both ways and returns the two entry files. */
function compile(name) {
  const source = path.join(here, 'cases', name);
  const dir = path.join(out, name);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(path.join(dir, 'original'), { recursive: true });
  const original = path.join(dir, 'original', 'input.js');
  writeFileSync(original, execFileSync(trioxide, ['strip', path.join(source, 'input.ts')], { encoding: 'utf8' }));
  const optimised = path.join(dir, 'optimised');
  execFileSync(trioxide, ['build', '--emit=js', '--no-wasm', '--root', source, '--out-dir', optimised, path.join(source, 'input.ts')]);
  return { original, optimised: path.join(optimised, 'input.js') };
}

function time1(flags, entry, mode) {
  const output = execFileSync(process.execPath, [...flags, path.join(here, 'worker.mjs'), entry, mode, time], {
    encoding: 'utf8',
  });
  return JSON.parse(output);
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

const rows = [];
for (const name of readdirSync(path.join(here, 'cases')).sort()) {
  if (process.env.CASE && !name.includes(process.env.CASE)) continue;
  const files = compile(name);
  const mode = /export const measure = "init"/.test(readFileSync(path.join(here, 'cases', name, 'input.ts'), 'utf8'))
    ? 'init'
    : 'bench';
  for (const engine of engines) {
    const samples = { original: [], optimised: [] };
    const ratios = [];
    for (let round = 0; round < rounds; round++) {
      const order = round % 2 === 0 ? ['original', 'optimised'] : ['optimised', 'original'];
      const results = {};
      for (const version of order) {
        results[version] = time1(engine.flags, files[version], mode);
        samples[version].push(results[version].mean);
      }
      assert.equal(results.optimised.check, results.original.check, `${name}: the versions disagree`);
      ratios.push(results.optimised.mean / results.original.mean);
    }
    rows.push({
      case: name,
      engine: engine.name,
      measures: mode === 'init' ? 'module evaluation' : 'bench()',
      'original (ops/s)': Math.round(median(samples.original)),
      'optimised (ops/s)': Math.round(median(samples.optimised)),
      speedup: `${median(ratios).toFixed(2)}×`,
      range: `${Math.min(...ratios).toFixed(2)}–${Math.max(...ratios).toFixed(2)}×`,
    });
  }
}
console.table(rows);
