// Runs every benchmark suite and writes one markdown report (docs/PLAN.md §8):
//
//   passes      each JS pass on its own case, stripped vs optimised (suites/passes.mjs)
//   programs    examples and realistic programs: stripped, trioxide, all passes, auto + WASM
//               (suites/programs.mjs)
//   partition   cost-model calibration kernels, JS vs WASM and the model's picks
//               (partition/run.mjs, which also writes partition/results.json for fit.mjs)
//   throughput  `trioxide build` wall time and peak memory (suites/throughput.mjs)
//
// Every runtime number comes from lib/harness.mjs: each variant in processes of its own,
// alternating order over ROUNDS rounds, median and range, under V8's defaults and cold
// (`--no-opt --no-maglev`; partition uses the defaults only).
//
// Usage: pnpm --filter trioxide-benchmarks bench [suite…] [--filter text] [--quick]
// Environment: TRIOXIDE_BIN (default target/release, else target/debug, else built),
// ROUNDS (default 5; 3 for partition and throughput), BENCH_TIME (milliseconds per process,
// default 1000), BENCH_OUT (report path, default .out/results.md).

import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { measure as partition } from './partition/run.mjs';
import * as passes from './suites/passes.mjs';
import * as programs from './suites/programs.mjs';
import * as throughput from './suites/throughput.mjs';
import { binary, out } from './lib/trioxide.mjs';

const args = process.argv.slice(2);
const filterAt = args.indexOf('--filter');
const filter = filterAt >= 0 ? args[filterAt + 1] : undefined;
const quick = args.includes('--quick');
const chosen = args.filter((arg, i) => !arg.startsWith('--') && args[i - 1] !== '--filter');
const rounds = Number(process.env.ROUNDS ?? (quick ? 1 : 5));
const budget = Number(process.env.BENCH_TIME ?? (quick ? 300 : 1000));
const trioxide = binary();

const suites = [
  passes,
  programs,
  {
    name: 'partition',
    title: 'Cost-model calibration (`partition/kernels/`, V8 defaults)',
    run: (options) => partition({ ...options, rounds: process.env.ROUNDS ? rounds : quick ? 1 : 3 }),
  },
  { ...throughput, run: (options) => throughput.run({ ...options, rounds: process.env.ROUNDS ? rounds : 3 }) },
];

function machine() {
  const cpu = os.cpus()[0]?.model ?? 'unknown CPU';
  let rustc = '';
  try {
    rustc = execFileSync('rustc', ['--version'], { encoding: 'utf8' }).trim();
  } catch {}
  let commit = '';
  try {
    commit = execFileSync('git', ['rev-parse', '--short', 'HEAD'], { encoding: 'utf8' }).trim();
  } catch {}
  return `Node ${process.version} (V8 ${process.versions.v8}), ${cpu} × ${os.cpus().length}, ${Math.round(os.totalmem() / 2 ** 30)} GiB, ${os.platform()} ${os.release()}; ${rustc}; trioxide ${path.relative(process.cwd(), trioxide)} at ${commit}`;
}

const sections = [`# trioxide benchmarks\n\n${machine()}\n\nSpeedups are against the first column (time ratio, median of rounds, range in parentheses).`];
const json = {};
for (const suite of suites) {
  if (chosen.length > 0 && !chosen.includes(suite.name)) continue;
  const { markdown, results } = suite.run({ trioxide, filter, rounds, budget, quick });
  json[suite.name] = results;
  sections.push(`## ${suite.title}\n\n${markdown}`);
}
const report = sections.join('\n\n') + '\n';
const file = process.env.BENCH_OUT ?? path.join(out, 'results.md');
mkdirSync(path.dirname(file), { recursive: true });
writeFileSync(file, report);
writeFileSync(file.replace(/\.md$/, '.json'), JSON.stringify(json, null, 2) + '\n');
process.stdout.write(report);
process.stderr.write(`\nwritten to ${file}\n`);
