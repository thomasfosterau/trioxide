// Default-mode reach (docs/PLAN.md §5 "For S8"): for every benchmark kernel and example,
// which top-level functions the Rust backend accepts with and without `trustTypes`, and the
// first rule that blocks each one by default.
//
// Usage: node reach.mjs

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const trioxide =
  process.env.TRIOXIDE_BIN ??
  ['release', 'debug'].map((p) => path.join(repo, 'target', p, 'trioxide')).find((p) => existsSync(p));
const out = path.join(here, '.out', 'reach');
mkdirSync(out, { recursive: true });

const programs = [
  ...readdirSync(path.join(here, 'kernels'))
    .sort()
    .map((name) => ({
      name: `kernels/${name}`,
      root: path.join(here, 'kernels', name),
      entry: 'input.ts',
      files: ['kernel.ts'],
    })),
  ...readdirSync(path.join(here, 'reach'))
    .sort()
    .map((name) => ({ name: `reach/${name}`, root: path.join(here, 'reach', name), entry: 'input.ts', files: ['kernel.ts'] })),
  ...readdirSync(path.join(repo, 'tests/islands'))
    .filter((name) => existsSync(path.join(repo, 'tests/islands', name, 'wasm', 'kernel.ts')))
    .sort()
    .map((name) => ({
      name: `tests/islands/${name}`,
      root: path.join(repo, 'tests/islands', name, 'wasm'),
      entry: 'input.ts',
      files: ['kernel.ts'],
      // Their kernels carry "use wasm"; strip it so the partitioner decides.
      undirect: true,
    })),
  { name: 'examples/mixed', root: path.join(repo, 'examples/mixed'), entry: 'src/main.ts', files: ['src/kernel.ts', 'src/ease.ts'] },
  { name: 'examples/image-filters', root: path.join(repo, 'examples/image-filters'), entry: 'src/main.ts', files: ['src/filters.ts', 'src/image.ts'] },
];

/** A copy of `program` without its `"use wasm"` directives. */
function undirected(program) {
  const dir = path.join(out, program.name.replace(/\//g, '_'));
  mkdirSync(dir, { recursive: true });
  for (const file of readdirSync(program.root).filter((f) => f.endsWith('.ts'))) {
    const text = readFileSync(path.join(program.root, file), 'utf8').replace(/^\s*"use wasm";\n/gm, '');
    writeFileSync(path.join(dir, file), text);
  }
  writeFileSync(path.join(dir, 'tsconfig.json'), readFileSync(path.join(here, 'tsconfig.json')));
  return { ...program, root: dir };
}

function decisions(program, trustTypes) {
  if (program.undirect) program = undirected(program);
  const config = path.join(out, 'trioxide.json');
  writeFileSync(config, JSON.stringify({ trustTypes }));
  const json = execFileSync(
    trioxide,
    ['explain', '--json', '--config', config, '--root', program.root, '--entry', path.join(program.root, program.entry)],
    { encoding: 'utf8' },
  );
  const report = JSON.parse(json);
  return report.modules
    .filter((m) => program.files.some((file) => m.path.endsWith(file)))
    .flatMap((m) => m.functions ?? []);
}

const eligible = (f) => f && !f.reason.startsWith('ineligible') && !f.reason.startsWith('nothing calls');
const lines = ['| program | function | default | trustTypes | blocked by default by |', '|---|---|---|---|---|'];
let counts = { default: 0, trusted: 0, total: 0 };
for (const program of programs) {
  const plain = decisions(program, false);
  const trusted = decisions(program, true);
  for (const f of plain) {
    const t = trusted.find((g) => g.name === f.name);
    counts.total++;
    if (eligible(f)) counts.default++;
    if (eligible(t)) counts.trusted++;
    const blocked = eligible(f) ? '' : f.reason.replace(/^ineligible: /, '');
    lines.push(
      `| ${program.name} | \`${f.name}\` | ${eligible(f) ? 'yes' : 'no'} | ${eligible(t) ? 'yes' : 'no'} | ${blocked} |`,
    );
  }
}
lines.push('', `Eligible: ${counts.default}/${counts.total} by default, ${counts.trusted}/${counts.total} with trustTypes.`);
process.stdout.write(lines.join('\n') + '\n');
