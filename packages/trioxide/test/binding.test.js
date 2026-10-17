import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'vitest';

import { spawnSync } from 'node:child_process';

import { Project, build, explain, findConfig, hasTsrs, loadConfig, transform } from 'trioxide';

test('transform strips types and returns a sourcemap', () => {
  const out = transform('const x: number = 1;\nexport { x };\n', '/virtual/a.ts');
  assert.equal(out.code, 'const x = 1;\nexport { x };\n');
  assert.equal(out.diagnostics.length, 0);
  assert.ok(out.map, 'sourcemap is on by default');
  assert.equal(JSON.parse(out.map).version, 3);
});

test('transform honours options', () => {
  const out = transform('export const x: number = 1;\n', '/virtual/a.ts', { sourcemap: false });
  assert.equal(out.map, undefined);
});

test('transform rejects unknown options', () => {
  assert.throws(() => transform('', '/virtual/a.ts', { mdoe: 'auto' }), /unknown field `mdoe`/);
});

test('transform reports syntax errors as diagnostics', () => {
  const out = transform('let x: = ;\n', '/virtual/a.ts');
  assert.equal(out.code, '');
  assert.equal(out.diagnostics[0].severity, 'error');
  assert.equal(out.diagnostics[0].labels[0].start, 7);
});

test('Project builds, updates and transforms', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'trioxide-'));
  writeFileSync(path.join(dir, 'main.ts'), 'import { two } from "./two.js";\nexport const four: number = two * 2;\n');
  writeFileSync(path.join(dir, 'two.ts'), 'export const two: number = 2;\n');

  const project = new Project(dir);
  const output = project.build([path.join(dir, 'main.ts')]);
  assert.deepEqual(
    output.modules.map((module) => path.basename(module.path)),
    ['main.ts', 'two.ts'],
  );
  assert.equal(output.report.typeOracle, hasTsrs ? 'tsrs' : 'noop');

  const invalidation = project.update(path.join(project.root, 'two.ts'), 'export const two: number = 3;\n');
  assert.equal(path.basename(invalidation.changed), 'two.ts');
  assert.deepEqual(invalidation.dependants.map((p) => path.basename(p)), ['main.ts']);
  assert.equal(project.transform(path.join(project.root, 'two.ts')).code, 'export const two = 3;\n');
  assert.equal(readFileSync(path.join(dir, 'two.ts'), 'utf8'), 'export const two: number = 2;\n');

  assert.equal(build([path.join(dir, 'main.ts')], { root: dir }).modules.length, 2);
  assert.equal(explain([path.join(dir, 'main.ts')], 'main.ts', { root: dir }).modules.length, 1);
});

test('config files are found up the tree and loaded as written', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'trioxide-'));
  mkdirSync(path.join(dir, '.git'));
  mkdirSync(path.join(dir, 'app/src'), { recursive: true });
  assert.equal(findConfig(path.join(dir, 'app/src')), null);
  writeFileSync(path.join(dir, 'trioxide.jsonc'), '{ // c\n "trustTypes": true, }');
  const found = findConfig(path.join(dir, 'app/src'));
  assert.equal(path.basename(found), 'trioxide.jsonc');
  assert.deepEqual(loadConfig(found), { trustTypes: true });
  writeFileSync(found, '{ "trustTypes": 1 }');
  assert.throws(() => loadConfig(found), /invalid type/);
});

test('the trioxide bin runs the CLI in the binding', () => {
  const bin = fileURLToPath(new URL('../bin/trioxide.js', import.meta.url));
  const version = spawnSync(process.execPath, [bin, '--version'], { encoding: 'utf8' });
  assert.equal(version.status, 0, version.stderr);
  assert.match(version.stdout, /^trioxide \d+\.\d+\.\d+/);

  const dir = mkdtempSync(path.join(tmpdir(), 'trioxide-'));
  writeFileSync(path.join(dir, 'a.ts'), 'export const a: number = 1;\n');
  const out = spawnSync(process.execPath, [bin, 'transform', path.join(dir, 'a.ts')], { encoding: 'utf8' });
  assert.equal(out.status, 0, out.stderr);
  assert.match(out.stdout, /^export const a = 1;\n/);

  const bad = spawnSync(process.execPath, [bin, 'nonsense'], { encoding: 'utf8' });
  assert.equal(bad.status, 2);
  assert.match(bad.stderr, /unrecognized subcommand/);
});
