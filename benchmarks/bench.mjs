// Runtime benchmarks of trioxide's JS passes (docs/PLAN.md §8): every case in cases/ is
// compiled twice, types stripped only (`trioxide strip`) and optimised (`trioxide build
// --emit=js --no-wasm`), and each version's `bench()` is timed with tinybench in a process of
// its own (worker.mjs), so one version's JIT state can't skew the other's. A case that exports
// `measure = "init"` is timed by evaluating the whole module (fresh imports) instead, which is
// where compile-time partial evaluation pays off.
//
// Every case runs under two engine configurations: V8's defaults, whose optimising tier
// already inlines small functions and replaces non-escaping objects in hot loops, and
// `--no-opt` (interpreter and baseline compiler only), which is where code runs before it is
// hot, or when it never gets hot.
//
// Set TRIOXIDE_BIN to use a prebuilt binary; otherwise `cargo build -p trioxide_cli` runs
// first. CASE filters cases by substring; BENCH_TIME sets the milliseconds per task (default
// 1000).

import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '..');
const out = path.join(here, '.out');
const time = process.env.BENCH_TIME ?? '1000';
const engines = [
  { name: 'default', flags: [] },
  { name: '--no-opt', flags: ['--no-opt'] },
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

const rows = [];
for (const name of readdirSync(path.join(here, 'cases')).sort()) {
  if (process.env.CASE && !name.includes(process.env.CASE)) continue;
  const files = compile(name);
  const mode = /export const measure = "init"/.test(readFileSync(path.join(here, 'cases', name, 'input.ts'), 'utf8'))
    ? 'init'
    : 'bench';
  for (const engine of engines) {
    const before = time1(engine.flags, files.original, mode);
    const after = time1(engine.flags, files.optimised, mode);
    assert.equal(after.check, before.check, `${name}: the versions disagree`);
    rows.push({
      case: name,
      engine: engine.name,
      measures: mode === 'init' ? 'module evaluation' : 'bench()',
      'original (ops/s)': Math.round(before.mean),
      'optimised (ops/s)': Math.round(after.mean),
      speedup: `${(after.mean / before.mean).toFixed(2)}×`,
      'rme %': `${before.rme.toFixed(1)} / ${after.rme.toFixed(1)}`,
    });
  }
}
console.table(rows);
