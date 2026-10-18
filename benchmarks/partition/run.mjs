// Calibrates the partitioner's cost model (docs/PLAN.md §5, §8 "Benchmarks").
//
// For every kernel in kernels/<name>/ (an input.ts whose exported `run()` does the work and
// returns a checksum), this:
//
//   1. builds it JS-only (`mode: "js-only"`),
//   2. builds it with every eligible cluster as an island (`wasm-preferred` with no
//      threshold), so the islands are exactly the ones the cost model scored,
//   3. times `run()` in a fresh Node process for each (after the islands are ready),
//   4. asks `trioxide explain --json` (auto mode, the default weights or `--weights`) what
//      it would pick, and with which numbers,
//   5. prints the timings, the model's verdicts and the confusion table, and writes
//      results.json for fit.mjs.
//
// Usage: node run.mjs [--filter name] [--weights file.json] [--quick]
// TRIOXIDE_BIN picks the trioxide binary (default: target/release, else target/debug).

import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const out = path.join(here, '.out');
const cacheDir = path.join(out, '.cache');

const args = process.argv.slice(2);
const option = (name) => {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
};
const filter = option('--filter');
const weights = option('--weights') ? JSON.parse(readFileSync(option('--weights'), 'utf8')) : {};
const quick = args.includes('--quick');

const trioxide =
  process.env.TRIOXIDE_BIN ??
  ['release', 'debug'].map((p) => path.join(repo, 'target', p, 'trioxide')).find((p) => existsSync(p));
if (!trioxide) throw new Error('build trioxide first (cargo build -p trioxide_cli --release)');

function config(dir, kernel, extra) {
  const own = ['trioxide.json', 'trioxide.jsonc']
    .map((name) => path.join(kernel, name))
    .filter((file) => existsSync(file))
    .map((file) => JSON.parse(readFileSync(file, 'utf8')))[0] ?? {};
  const file = path.join(dir, 'trioxide.json');
  writeFileSync(file, JSON.stringify({ ...own, cacheDir, ...extra, costModel: { ...own.costModel, ...extra.costModel } }));
  return file;
}

function build(kernel, dir, extra) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  writeFileSync(path.join(dir, 'package.json'), '{ "type": "module" }\n');
  const file = config(path.join(dir, '..'), kernel, extra);
  const result = spawnSync(
    trioxide,
    ['build', '--emit=js', '--require-wasm', '--config', file, '--root', kernel, '--out-dir', dir, path.join(kernel, 'input.ts')],
    { encoding: 'utf8' },
  );
  if (result.status !== 0) throw new Error(`build of ${kernel} failed:\n${result.stderr}`);
  const islands = path.join(dir, '_islands');
  return existsSync(islands)
    ? readdirSync(islands).filter((name) => name.endsWith('.js')).map((name) => path.join(islands, name))
    : [];
}

/**
 * Times `run()` of `entry` in fresh processes (three, one with --quick): the median of their
 * medians, since timings vary by ±10% from one process to the next.
 */
function time(entry, glues) {
  const runs = Array.from({ length: quick ? 1 : 3 }, () => timeOnce(entry, glues));
  runs.sort((a, b) => a.ns - b.ns);
  return runs[runs.length >> 1];
}

/** Times `run()` of `entry` in a fresh process, after the `glues` are ready. */
function timeOnce(entry, glues) {
  const script = `
const started = performance.now();
const m = await import(${JSON.stringify(pathToFileURL(entry).href)});
for (const glue of ${JSON.stringify(glues.map((g) => pathToFileURL(g).href))}) await (await import(glue)).ready();
const readyMs = performance.now() - started;
let result = m.run();
const warmUntil = performance.now() + ${quick ? 100 : 400};
let warm = 0;
while (performance.now() < warmUntil || warm < 5) { m.run(); warm++; }
// Batches of at least 25ms; the median batch gives the time per run.
let perBatch = 1;
for (;;) {
  const t0 = performance.now();
  for (let i = 0; i < perBatch; i++) m.run();
  if (performance.now() - t0 >= 25) break;
  perBatch *= 2;
}
const samples = [];
for (let b = 0; b < ${quick ? 5 : 15}; b++) {
  const t0 = performance.now();
  for (let i = 0; i < perBatch; i++) result = m.run();
  samples.push(((performance.now() - t0) * 1e6) / perBatch);
}
samples.sort((a, b) => a - b);
process.stdout.write(JSON.stringify({ result: String(result), ns: samples[samples.length >> 1], readyMs }));
`;
  const result = spawnSync(process.execPath, ['--input-type=module', '--eval', script], {
    encoding: 'utf8',
    cwd: path.dirname(entry),
  });
  if (result.status !== 0) throw new Error(`${entry} failed:\n${result.stderr}`);
  return JSON.parse(result.stdout);
}

function explain(kernel, dir, model) {
  const file = config(dir, kernel, { mode: 'auto', costModel: model });
  const json = execFileSync(
    trioxide,
    ['explain', '--json', '--config', file, '--root', kernel, '--entry', path.join(kernel, 'input.ts'), path.join(kernel, 'kernel.ts')],
    { encoding: 'utf8' },
  );
  return JSON.parse(json).modules.flatMap((m) => m.functions ?? []);
}

/** The kernel's cluster: the scored function whose island does the most work. */
function cluster(functions) {
  const scored = functions.filter((f) => f.cost);
  return scored.length === 0 ? null : scored.reduce((a, b) => (b.cost.work > a.cost.work ? b : a));
}

const ZERO = {
  arithmetic: 0, integer: 0, conversion: 0, wrapping: 0, math: 0, remainder: 0, index: 0, checkedIndex: 0, iteration: 0,
  crossing: 0, aggregate: 0, byte: 0, instantiation: 0,
};
const FEATURES = ['arithmetic', 'integer', 'conversion', 'wrapping', 'math', 'remainder', 'index', 'checkedIndex', 'iteration'];

/**
 * The cost model's verdict on the kernel (auto mode, the default weights or `--weights`),
 * and the cluster's features for fit.mjs: operation counts per kind (explain with one weight
 * set to 1 at a time), calls and bytes.
 */
function verdict(kernel, dir) {
  const functions = explain(kernel, dir, weights);
  const ineligible = functions
    .filter((f) => f.reason.startsWith('ineligible'))
    .map((f) => `${f.name}: ${f.reason.replace(/^ineligible: /, '')}`);
  const best = cluster(functions);
  if (!best) return { wasm: false, ineligible };
  const features = { calls: best.cost.calls, bytes: best.cost.bytes };
  for (const feature of FEATURES) {
    const one = functions.length && cluster(explain(kernel, dir, { ...ZERO, [feature]: 1 }).filter((f) => f.name === best.name));
    features[feature] = one?.cost.benefit ?? 0;
  }
  const aggregates = cluster(explain(kernel, dir, { ...ZERO, aggregate: 1 }).filter((f) => f.name === best.name));
  features.aggregates = aggregates?.cost.marshalling ?? 0;
  return { wasm: best.cost.benefit - best.cost.cost > best.cost.threshold, cost: best.cost, function: best.name, features, ineligible };
}

const kernels = readdirSync(path.join(here, 'kernels'))
  .filter((name) => !filter || name.includes(filter))
  .sort();
const results = [];
for (const name of kernels) {
  const kernel = path.join(here, 'kernels', name);
  const work = path.join(out, name);
  build(kernel, path.join(work, 'js'), { mode: 'js-only' });
  const glues = build(kernel, path.join(work, 'wasm'), {
    mode: 'wasm-preferred',
    costModel: { wasmPreferredThreshold: -1e300 },
  });
  const js = time(path.join(work, 'js', 'input.js'), []);
  const wasm = glues.length > 0 ? time(path.join(work, 'wasm', 'input.js'), glues) : null;
  if (wasm && wasm.result !== js.result) throw new Error(`${name}: JS printed ${js.result}, WASM ${wasm.result}`);
  const model = verdict(kernel, work);
  results.push({ name, js: js.ns, wasm: wasm?.ns ?? null, readyMs: wasm?.readyMs ?? null, model });
  process.stderr.write(`${name}: done\n`);
}

const fmt = (ns) => (ns == null ? '—' : ns >= 1e6 ? `${(ns / 1e6).toFixed(2)} ms` : `${(ns / 1e3).toFixed(1)} µs`);
const lines = ['| kernel | JS | WASM | faster | model picks | benefit − cost | agrees |', '|---|---|---|---|---|---|---|'];
// Within 5% either way is a tie: both sides are right.
const confusion = { wasm: { wasm: 0, js: 0 }, tie: { wasm: 0, js: 0 }, js: { wasm: 0, js: 0 } };
for (const r of results) {
  if (r.wasm == null) {
    lines.push(`| ${r.name} | ${fmt(r.js)} | not eligible | — | JS | — | — |`);
    continue;
  }
  const ratio = Math.max(r.js, r.wasm) / Math.min(r.js, r.wasm);
  const faster = ratio < 1.05 ? 'tie' : r.wasm < r.js ? 'wasm' : 'js';
  const picks = r.model.wasm ? 'wasm' : 'js';
  confusion[faster][picks]++;
  const net = r.model.cost ? Math.round(r.model.cost.benefit - r.model.cost.cost) : '—';
  const label = `${r.wasm < r.js ? 'WASM' : 'JS'} ×${ratio.toFixed(2)}${faster === 'tie' ? ' (tie)' : ''}`;
  const agrees = faster === 'tie' || faster === picks ? 'yes' : '**no**';
  lines.push(`| ${r.name} | ${fmt(r.js)} | ${fmt(r.wasm)} | ${label} | ${picks.toUpperCase()} | ${net} | ${agrees} |`);
}
lines.push(
  '',
  '| measured faster ↓ / model picks → | WASM | JS |',
  '|---|---|---|',
  `| WASM | ${confusion.wasm.wasm} | ${confusion.wasm.js} |`,
  `| tie (within 5%) | ${confusion.tie.wasm} | ${confusion.tie.js} |`,
  `| JS | ${confusion.js.wasm} | ${confusion.js.js} |`,
);
process.stdout.write(lines.join('\n') + '\n');
writeFileSync(path.join(here, 'results.json'), JSON.stringify(results, null, 2) + '\n');
