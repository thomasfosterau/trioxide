// Runtime benchmarks (docs/PLAN.md §8): every case in cases/ is compiled twice, types stripped
// only (`trioxide strip`) and optimised (`trioxide build --emit=js --no-wasm`, app world), and
// each case's exported `bench()` is timed in both with tinybench, after checking they return
// the same value.
//
// Set TRIOXIDE_BIN to use a prebuilt binary; otherwise `cargo build -p trioxide_cli` runs
// first. Set CASE to a substring to run only matching cases, and TIME to the milliseconds each
// task runs for (default 1000). Prints a Markdown table.

import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { Bench } from 'tinybench';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '..');
const casesDir = path.join(here, 'cases');
const outDir = path.join(here, '.out');

let trioxide = process.env.TRIOXIDE_BIN;
if (!trioxide) {
  execFileSync('cargo', ['build', '--quiet', '-p', 'trioxide_cli'], { cwd: repo, stdio: 'inherit' });
  trioxide = path.join(repo, 'target/debug/trioxide');
}

function run(args) {
  return execFileSync(trioxide, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
}

function prepare(dir) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  writeFileSync(path.join(dir, 'package.json'), '{ "type": "module" }\n');
}

const cases = readdirSync(casesDir)
  .sort()
  .filter((name) => !process.env.CASE || name.includes(process.env.CASE));
const time = Number(process.env.TIME ?? 1000);

const rows = [];
for (const name of cases) {
  const fixture = path.join(casesDir, name);
  const entry = path.join(fixture, 'input.ts');

  const original = path.join(outDir, name, 'original');
  prepare(original);
  writeFileSync(path.join(original, 'input.js'), run(['strip', '--root', fixture, entry]));

  const optimised = path.join(outDir, name, 'trioxide');
  prepare(optimised);
  run(['build', '--emit=js', '--no-wasm', '--root', fixture, '--out-dir', optimised, entry]);

  const a = await import(pathToFileURL(path.join(original, 'input.js')).href);
  const b = await import(pathToFileURL(path.join(optimised, 'input.js')).href);
  assert.deepEqual(b.bench(), a.bench(), `${name}: the optimised build computes something else`);

  const bench = new Bench({ name, time, warmup: true });
  bench.add('original', () => a.bench());
  bench.add('trioxide', () => b.bench());
  await bench.run();
  const [before, after] = bench.tasks.map((task) => task.result.throughput.mean);
  rows.push({ name, before, after });
  process.stderr.write(`${name}: ${(after / before).toFixed(2)}×\n`);
}

const format = (hz) => Math.round(hz).toLocaleString('en-US');
console.log('| case | original (ops/s) | trioxide (ops/s) | speedup |');
console.log('|---|---:|---:|---:|');
for (const { name, before, after } of rows) {
  console.log(`| ${name} | ${format(before)} | ${format(after)} | ${(after / before).toFixed(2)}× |`);
}
