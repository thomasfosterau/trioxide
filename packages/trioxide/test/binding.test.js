import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { test } from 'node:test';

import { Project, build, explain, optionsSchema, transform } from 'trioxide';
import trioxideVite from 'trioxide/vite';

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
  assert.equal(output.report.typeOracle, 'noop');

  const invalidation = project.update(path.join(project.root, 'two.ts'), 'export const two: number = 3;\n');
  assert.equal(path.basename(invalidation.changed), 'two.ts');
  assert.deepEqual(invalidation.dependants.map((p) => path.basename(p)), ['main.ts']);
  assert.equal(project.transform(path.join(project.root, 'two.ts')).code, 'export const two = 3;\n');
  assert.equal(readFileSync(path.join(dir, 'two.ts'), 'utf8'), 'export const two: number = 2;\n');

  assert.equal(build([path.join(dir, 'main.ts')], { root: dir }).modules.length, 2);
  assert.equal(explain([path.join(dir, 'main.ts')], 'main.ts', { root: dir }).modules.length, 1);
});

test('the declared options cover the schema', () => {
  const declared = readFileSync(new URL('../src/index.d.ts', import.meta.url), 'utf8');
  const options = declared.slice(declared.indexOf('interface TrioxideOptions'), declared.indexOf('interface FunctionDecision'));
  for (const key of Object.keys(optionsSchema().properties)) {
    const name = key.replace('$', '\\$');
    assert.match(options, new RegExp(`^\\s+${name}\\?:`, 'm'), `TrioxideOptions lacks ${key}`);
  }
});

test('the vite plugin is a stub', () => {
  assert.throws(() => trioxideVite(), /not yet implemented/);
});
