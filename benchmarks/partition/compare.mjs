// Times a program's exported `run()` built three ways: JS only, `auto` (the islands the cost
// model picks) and every eligible cluster as an island. The examples' READMEs use it.
//
// Usage (from a project directory): node <repo>/benchmarks/partition/compare.mjs src/main.ts
// TRIOXIDE_BIN picks the trioxide binary (default: target/release, else target/debug).

import { existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const trioxide =
  process.env.TRIOXIDE_BIN ??
  ['release', 'debug'].map((p) => path.join(repo, 'target', p, 'trioxide')).find((p) => existsSync(p));
const entry = path.resolve(process.argv[2] ?? 'src/main.ts');
const root = process.cwd();
const out = path.join(root, 'node_modules', '.trioxide-compare');

const variants = [
  ['JS only', { mode: 'js-only' }],
  ['auto', { mode: 'auto' }],
  ['every cluster', { mode: 'wasm-preferred', costModel: { wasmPreferredThreshold: -1e300 } }],
];
const rows = [];
for (const [name, options] of variants) {
  const dir = path.join(out, name.replace(/ /g, '-'));
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  writeFileSync(path.join(dir, 'package.json'), '{ "type": "module" }\n');
  const config = path.join(out, `${name.replace(/ /g, '-')}.json`);
  writeFileSync(config, JSON.stringify({ ...options, cacheDir: path.join(out, '.cache') }));
  const build = spawnSync(trioxide, ['build', '--config', config, '--root', root, '--out-dir', dir, entry], {
    encoding: 'utf8',
  });
  if (build.status !== 0) throw new Error(build.stderr);
  const islands = path.join(dir, '_islands');
  const glues = existsSync(islands) ? readdirSync(islands).filter((f) => f.endsWith('.js')) : [];
  const module = path.join(dir, path.relative(root, entry)).replace(/\.ts$/, '.js');
  const script = `
const m = await import(${JSON.stringify(pathToFileURL(module).href)});
for (const g of ${JSON.stringify(glues.map((g) => pathToFileURL(path.join(islands, g)).href))}) await (await import(g)).ready();
const result = JSON.stringify(m.run());
const until = performance.now() + 1000;
while (performance.now() < until) m.run();
const samples = [];
for (let i = 0; i < 41; i++) { const t = performance.now(); m.run(); samples.push(performance.now() - t); }
samples.sort((a, b) => a - b);
process.stdout.write(JSON.stringify({ result, ms: samples[20] }));
`;
  const run = spawnSync(process.execPath, ['--input-type=module', '--eval', script], { encoding: 'utf8', cwd: dir });
  if (run.status !== 0) throw new Error(run.stderr);
  const { result, ms } = JSON.parse(run.stdout);
  rows.push({ name, islands: glues.map((g) => g.replace(/\.js$/, '')).join(', ') || '—', ms, result });
}
const same = rows.every((r) => r.result === rows[0].result);
process.stdout.write('| build | islands | run() |\n|---|---|---|\n');
for (const r of rows) process.stdout.write(`| ${r.name} | ${r.islands} | ${r.ms.toFixed(2)} ms |\n`);
process.stdout.write(same ? '\nAll three print the same results.\n' : '\n**The results differ!**\n');
if (!same) process.exitCode = 1;
