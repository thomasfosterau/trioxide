// Whole programs: every example with a `run()` export and the realistic programs in
// programs/<name>/, each timed four ways: types stripped only, trioxide's JS with the default
// passes (no WASM), trioxide's JS with every pass on (`inline-fn` and `scalar` too), and the
// default configuration (`mode: "auto"`) with the islands the cost model picks.

import { existsSync, readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { compare, formatSpeedup, formatTime, table } from '../lib/harness.mjs';
import { ALL_PASSES, benchmarks, build, jsName, out, repo, strip } from '../lib/trioxide.mjs';

export const name = 'programs';
export const title = 'Whole programs (`programs/`, `examples/`)';

/** Every program: `{ name, root, entry }`, the entry exporting `run()`. */
export function programs() {
  const own = readdirSync(path.join(benchmarks, 'programs'))
    .sort()
    .map((name) => ({ name, root: path.join(benchmarks, 'programs', name), entry: 'input.ts' }));
  const examples = readdirSync(path.join(repo, 'examples'))
    .sort()
    .filter((name) => existsSync(path.join(repo, 'examples', name, 'src', 'main.ts')))
    .filter((name) => /export (const|function) run\b/.test(readFileSync(path.join(repo, 'examples', name, 'src', 'main.ts'), 'utf8')))
    .map((name) => ({ name: `examples/${name}`, root: path.join(repo, 'examples', name), entry: 'src/main.ts' }));
  return [...own, ...examples];
}

function ownConfig(root) {
  for (const file of ['trioxide.json', 'trioxide.jsonc']) {
    const full = path.join(root, file);
    if (existsSync(full)) return JSON.parse(readFileSync(full, 'utf8').replace(/^\s*\/\/.*$/gm, ''));
  }
  return {};
}

export function run({ trioxide, filter, rounds, budget }) {
  const rows = [];
  const results = [];
  for (const program of programs()) {
    if (filter && !`programs/${program.name}`.includes(filter)) continue;
    const dir = path.join(out, 'programs', program.name.replace('/', '-'));
    const own = ownConfig(program.root);
    strip(trioxide, program.root, path.join(dir, 'stripped'));
    build(trioxide, program.root, program.entry, path.join(dir, 'trioxide'), { wasm: { enabled: false } }, own);
    build(trioxide, program.root, program.entry, path.join(dir, 'all-passes'), { wasm: { enabled: false }, passes: { ...own.passes, ...ALL_PASSES } }, own);
    const glues = build(trioxide, program.root, program.entry, path.join(dir, 'auto'), { mode: 'auto' }, own);
    const entry = jsName(program.entry);
    const variants = [
      { name: 'stripped', entry: path.join(dir, 'stripped', entry) },
      { name: 'trioxide', entry: path.join(dir, 'trioxide', entry) },
      { name: 'all passes', entry: path.join(dir, 'all-passes', entry) },
      { name: 'auto + WASM', entry: path.join(dir, 'auto', entry), glues },
    ];
    const islands = glues.map((glue) => path.basename(glue, '.js')).join(', ') || '—';
    for (const row of compare(variants, { rounds, budget, label: program.name })) {
      const [stripped, ...others] = row.variants;
      results.push({ program: program.name, engine: row.engine, islands, variants: row.variants });
      rows.push([`\`${program.name}\``, row.engine, formatTime(stripped.ns), ...others.map(formatSpeedup), islands]);
    }
    process.stderr.write(`programs/${program.name}: done\n`);
  }
  return {
    results,
    markdown: table(['program', 'engine', 'stripped', 'trioxide', 'all passes', 'auto + WASM', 'islands'], rows),
  };
}
