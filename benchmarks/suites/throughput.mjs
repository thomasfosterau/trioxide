// Compiler throughput: `trioxide build` wall time and peak resident memory on the conformance
// corpus (tests/conformance, every library module and driver an entry, default options) and on
// the analysis benchmark's corpus (`cargo bench -p trioxide_analysis`: the tsrs span corpus and
// every analysis fixture, 20 copies, each module an entry; 1820 modules when S2 measured it). Each build runs ROUNDS
// times from a cold process; the table gives the median and range. Peak memory comes from
// `wait4` (python3).

import { spawnSync } from 'node:child_process';
import { cpSync, existsSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { formatTime, median, table } from '../lib/harness.mjs';
import { benchmarks, cacheDir, out, prepare, repo, sources } from '../lib/trioxide.mjs';

export const name = 'throughput';
export const title = 'Compiler throughput (`trioxide build`)';

const COPIES = 20;

/** The analysis benchmark's corpus, copied COPIES times (directory trees kept, so imports resolve). */
function analysisCorpus() {
  const root = path.join(out, 'throughput', 'analysis-corpus');
  if (!existsSync(path.join(root, `c${COPIES - 1}`))) {
    rmSync(root, { recursive: true, force: true });
    for (let copy = 0; copy < COPIES; copy++) {
      for (const dir of ['crates/trioxide_types/tests/corpus', 'tests/fixtures/analysis']) {
        cpSync(path.join(repo, dir), path.join(root, `c${copy}`, dir), {
          recursive: true,
          filter: (file) => statSync(file).isDirectory() || file.endsWith('.ts'),
        });
      }
    }
  }
  return root;
}

function corpora() {
  const conformance = path.join(repo, 'tests', 'conformance');
  const libraries = readdirSync(conformance)
    .filter((name) => existsSync(path.join(conformance, name, 'main.ts')))
    .sort()
    .map((name) => {
      const root = path.join(conformance, name);
      const tests = new Set(['test', 'tests']);
      const files = sources(root).filter((file) => !tests.has(file.split(path.sep)[0]));
      return { name: `conformance/${name}`, root, entries: files };
    });
  const analysis = analysisCorpus();
  const files = sources(analysis);
  return [...libraries, { name: `analysis bench corpus (${files.length} modules)`, root: analysis, entries: files }];
}

function measure(trioxide, corpus, dir) {
  prepare(dir);
  const config = `${dir}.json`;
  writeFileSync(config, JSON.stringify({ cacheDir }));
  const args = ['build', '--config', config, '--root', corpus.root, '--out-dir', dir, ...corpus.entries.map((file) => path.join(corpus.root, file))];
  const result = spawnSync('python3', [path.join(benchmarks, 'lib', 'measure.py'), trioxide, ...args], { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 });
  if (result.status !== 0) throw new Error(`measure.py failed (is python3 installed?):\n${result.stderr}`);
  const lines = result.stderr.trimEnd().split('\n');
  const stats = JSON.parse(lines.at(-1));
  if (stats.status !== 0) throw new Error(`trioxide build of ${corpus.name} failed:\n${lines.slice(0, -1).join('\n')}`);
  return stats;
}

export function run({ trioxide, filter, rounds }) {
  const rows = [];
  const results = [];
  for (const corpus of corpora()) {
    if (filter && !`throughput/${corpus.name}`.includes(filter)) continue;
    const dir = path.join(out, 'throughput', corpus.name.replace(/[^\w-]+/g, '-'));
    const runs = [];
    for (let round = 0; round < Math.max(rounds, 3); round++) runs.push(measure(trioxide, corpus, dir));
    const ms = runs.map((r) => r.ms);
    const rss = runs.map((r) => r.rss / 1024);
    results.push({ corpus: corpus.name, modules: corpus.entries.length, ms, rss });
    rows.push([
      corpus.name,
      String(corpus.entries.length),
      `${formatTime(median(ms) * 1e6)} (${formatTime(Math.min(...ms) * 1e6)}–${formatTime(Math.max(...ms) * 1e6)})`,
      `${median(rss).toFixed(0)} MiB (${Math.min(...rss).toFixed(0)}–${Math.max(...rss).toFixed(0)})`,
    ]);
    process.stderr.write(`throughput/${corpus.name}: done\n`);
  }
  return { results, markdown: table(['corpus', 'modules', 'wall time', 'peak RSS'], rows) };
}
