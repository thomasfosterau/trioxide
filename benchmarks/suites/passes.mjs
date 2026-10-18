// The JS passes, one case each (passes/cases/<name>/input.ts, which exports `bench()`): types
// stripped against `trioxide build --no-wasm` with the case's own trioxide.json (which turns on
// the passes that are off by default). A case that exports `measure = "init"` is timed by
// evaluating the module afresh, where compile-time partial evaluation pays off.

import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { compare, formatSpeedup, formatTime, table } from '../lib/harness.mjs';
import { benchmarks, build, out, strip } from '../lib/trioxide.mjs';

export const name = 'passes';
export const title = 'JS passes (`passes/cases/`)';

export function run({ trioxide, filter, rounds, budget }) {
  const rows = [];
  const results = [];
  for (const name of readdirSync(path.join(benchmarks, 'passes', 'cases')).sort()) {
    if (filter && !`passes/${name}`.includes(filter)) continue;
    const root = path.join(benchmarks, 'passes', 'cases', name);
    const dir = path.join(out, 'passes', name);
    const source = readFileSync(path.join(root, 'input.ts'), 'utf8');
    const mode = /export const measure = "init"/.test(source) ? 'init' : 'run';
    strip(trioxide, root, path.join(dir, 'stripped'));
    let own = {};
    try {
      own = JSON.parse(readFileSync(path.join(root, 'trioxide.json'), 'utf8'));
    } catch {}
    build(trioxide, root, 'input.ts', path.join(dir, 'optimised'), { wasm: { enabled: false } }, own);
    const variants = ['stripped', 'optimised'].map((variant) => ({
      name: variant,
      entry: path.join(dir, variant, 'input.js'),
      export: 'bench',
      mode,
    }));
    for (const row of compare(variants, { rounds, budget, label: name })) {
      const [stripped, optimised] = row.variants;
      results.push({ case: name, engine: row.engine, measures: mode, stripped: stripped.ns, optimised: optimised.ns, speedup: optimised.speedup, low: optimised.low, high: optimised.high });
      rows.push([`\`${name}\``, row.engine, mode === 'init' ? 'module evaluation' : '`bench()`', formatTime(stripped.ns), formatTime(optimised.ns), formatSpeedup(optimised)]);
    }
    process.stderr.write(`passes/${name}: done\n`);
  }
  return {
    results,
    markdown: table(['case', 'engine', 'measures', 'stripped', 'optimised', 'speedup (range)'], rows),
  };
}
