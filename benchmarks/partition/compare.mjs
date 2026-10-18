// Times a program's exported `run()` built three ways: JS only, `auto` (the islands the cost
// model picks) and every eligible cluster as an island, with the shared harness
// (lib/harness.mjs: fresh processes, alternating order, V8's defaults). The examples' READMEs
// use it.
//
// Usage (from a project directory): node <repo>/benchmarks/partition/compare.mjs src/main.ts
// TRIOXIDE_BIN picks the trioxide binary (default: target/release, else target/debug).

import path from 'node:path';
import { compare, ENGINES, formatTime } from '../lib/harness.mjs';
import { binary, build } from '../lib/trioxide.mjs';

const trioxide = binary();
const root = process.cwd();
const entry = path.relative(root, path.resolve(process.argv[2] ?? 'src/main.ts'));
const out = path.join(root, 'node_modules', '.trioxide-compare');

const builds = [
  ['JS only', { mode: 'js-only' }],
  ['auto', { mode: 'auto' }],
  ['every cluster', { mode: 'wasm-preferred', costModel: { wasmPreferredThreshold: -1e300 } }],
];
const variants = builds.map(([name, options]) => {
  const dir = path.join(out, name.replace(/ /g, '-'));
  const glues = build(trioxide, root, entry, dir, options);
  return { name, entry: path.join(dir, entry.replace(/\.ts$/, '.js')), glues };
});
const [row] = compare(variants, { engines: [ENGINES[0]], rounds: 3, label: entry });
process.stdout.write('| build | islands | run() |\n|---|---|---|\n');
row.variants.forEach((v, i) => {
  const islands = variants[i].glues.map((g) => path.basename(g, '.js')).join(', ') || '—';
  process.stdout.write(`| ${v.name} | ${islands} | ${formatTime(v.ns)} |\n`);
});
process.stdout.write('\nAll three compute the same results.\n');
