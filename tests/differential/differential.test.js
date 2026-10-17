// The differential suite (docs/PLAN.md §8): each fixture whose entry exports `main()` is
// compiled three ways and run in Node; all variants must print exactly the same stdout.
//
//   a. types stripped only (`trioxide strip`, plain oxc_transformer)
//   b. trioxide's optimised JS (`trioxide build --emit=js`)
//   c. trioxide JS + WASM islands (skipped until the WASM backend lands)
//
// Set TRIOXIDE_BIN to use a prebuilt binary; otherwise `cargo build -p trioxide_cli` runs first.
// Set FIXTURE to a substring to run only matching fixtures.

import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { before, describe, test } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const fixturesDir = path.join(repo, 'tests/fixtures');
// Inside this package so outputs resolve its node_modules (e.g. @oxc-project/runtime helpers).
const outDir = path.join(here, '.out');

const SOURCE_EXTENSIONS = new Set(['.ts', '.tsx', '.mts', '.cts']);

function discover(dir, found = []) {
  const names = readdirSync(dir).sort();
  const entry = ['input.ts', 'input.tsx'].map((name) => path.join(dir, name)).find((p) => existsSync(p));
  if (entry) found.push(entry);
  for (const name of names) {
    const child = path.join(dir, name);
    if (name !== 'node_modules' && statSync(child).isDirectory()) discover(child, found);
  }
  return found;
}

function sourcesOf(dir) {
  return readdirSync(dir).filter((name) => SOURCE_EXTENSIONS.has(path.extname(name)) && !name.endsWith('.d.ts'));
}

function jsName(file) {
  const ext = path.extname(file);
  const js = ext === '.mts' ? '.mjs' : ext === '.cts' ? '.cjs' : '.js';
  return file.slice(0, -ext.length) + js;
}

let trioxide = process.env.TRIOXIDE_BIN;

function run(args) {
  return execFileSync(trioxide, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
}

/** Imports `entry` in a fresh Node process, awaits its `main()` and returns stdout. */
function execute(entry) {
  const script = `const m = await import(${JSON.stringify(pathToFileURL(entry).href)});
if (typeof m.main !== 'function') { process.stderr.write('no main()'); process.exit(3); }
await m.main();`;
  const result = spawnSync(process.execPath, ['--input-type=module', '--eval', script], {
    encoding: 'utf8',
    cwd: path.dirname(entry),
    timeout: 30_000,
  });
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

function prepare(dir) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  writeFileSync(path.join(dir, 'package.json'), '{ "type": "module" }\n');
}

before(() => {
  if (!trioxide) {
    execFileSync('cargo', ['build', '--quiet', '-p', 'trioxide_cli'], { cwd: repo, stdio: 'inherit' });
    trioxide = path.join(repo, 'target/debug/trioxide');
  }
});

const entries = discover(fixturesDir).filter((entry) => !process.env.FIXTURE || entry.includes(process.env.FIXTURE));

for (const entry of entries) {
  const fixture = path.dirname(entry);
  const name = path.relative(fixturesDir, fixture);
  const work = path.join(outDir, name);

  describe(name, () => {
    let stripped;

    test('a: types stripped', () => {
      const dir = path.join(work, 'a');
      prepare(dir);
      for (const file of sourcesOf(fixture)) {
        writeFileSync(path.join(dir, jsName(file)), run(['strip', '--root', fixture, path.join(fixture, file)]));
      }
      stripped = execute(path.join(dir, jsName(path.basename(entry))));
      assert.equal(stripped.status, 0, `stripped variant failed:\n${stripped.stderr}`);
    });

    test('b: trioxide --emit=js', (t) => {
      if (stripped?.status !== 0) return t.skip('baseline failed');
      const dir = path.join(work, 'b');
      prepare(dir);
      run(['build', '--emit=js', '--root', fixture, '--out-dir', dir, entry]);
      const optimised = execute(path.join(dir, jsName(path.basename(entry))));
      assert.equal(optimised.status, 0, `trioxide variant failed:\n${optimised.stderr}`);
      assert.equal(optimised.stdout, stripped.stdout);
    });

    test('c: trioxide JS+WASM', { skip: 'WASM islands are not implemented yet (S7)' }, () => {});
  });
}
