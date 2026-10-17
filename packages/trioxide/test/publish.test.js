import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

import { test } from 'vitest';

import { stage } from '../scripts/publish.js';

const manifest = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));

test('there is one platform package per napi target', () => {
  const dirs = readdirSync(new URL('../npm', import.meta.url));
  assert.equal(dirs.length, manifest.napi.targets.length);
  for (const dir of dirs) {
    const platform = JSON.parse(readFileSync(new URL(`../npm/${dir}/package.json`, import.meta.url), 'utf8'));
    assert.equal(platform.name, `${manifest.napi.packageName}-${dir}`);
    assert.equal(platform.main, `trioxide.${dir}.node`);
    assert.deepEqual(platform.files, [platform.main]);
  }
});

test('staging versions every package and lists the bindings as optional dependencies', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'trioxide-publish-'));
  mkdirSync(path.join(dir, 'artifacts/binding-linux-x64-gnu'), { recursive: true });
  writeFileSync(path.join(dir, 'artifacts/binding-linux-x64-gnu/trioxide.linux-x64-gnu.node'), 'fake');
  const options = { version: '1.2.3-rc.1', artifacts: path.join(dir, 'artifacts'), out: path.join(dir, 'out') };
  assert.throws(() => stage(options), /no trioxide\.darwin-arm64\.node/);

  const staged = stage({ ...options, allowMissing: true });
  const main = JSON.parse(readFileSync(path.join(staged.main, 'package.json'), 'utf8'));
  assert.equal(main.version, '1.2.3-rc.1');
  assert.equal(main.devDependencies, undefined);
  assert.equal(Object.keys(main.optionalDependencies).length, manifest.napi.targets.length);
  assert.equal(main.optionalDependencies['@trioxide/binding-linux-x64-gnu'], '1.2.3-rc.1');
  assert.deepEqual(
    staged.platforms.filter((platform) => platform.present).map((platform) => platform.name),
    ['@trioxide/binding-linux-x64-gnu'],
  );

  const packed = JSON.parse(execFileSync('npm', ['pack', '--dry-run', '--json'], { cwd: staged.main, encoding: 'utf8' }));
  const files = packed[0].files.map((file) => file.path).sort();
  for (const expected of ['bin/trioxide.js', 'binding/index.cjs', 'src/vite.js', 'src/options.d.ts', 'schema.json', 'LICENSE']) {
    assert.ok(files.includes(expected), `${expected} missing from ${files.join(', ')}`);
  }
  assert.ok(!files.some((file) => file.endsWith('.node')), 'the main package ships no binary');
});
