// The differential suite (docs/PLAN.md §8): each fixture whose entry exports `main()` is
// compiled three ways and run in Node; all variants must print exactly the same stdout.
//
//   a. types stripped only (`trioxide strip`, plain oxc_transformer)
//   b. trioxide's optimised JS (`trioxide build --emit=js`)
//   c. trioxide JS + WASM islands, once with top-level-await glue and once with `ready()` glue
//
// Fixtures come from tests/fixtures and tests/islands. Until the partitioner chooses islands
// (S8), leg c runs on fixtures with a hand-written island: `island/` holds its `island.json`
// and Rust crate, and `kernel.ts` holds exactly the functions it replaces. The leg builds the
// island with `--debug-island`, keeps the JS kernel as `kernel.fallback.js` (the spec's
// aliasing fallback), and swaps `kernel.js` for a re-export of the island's glue.
//
// Set TRIOXIDE_BIN to use a prebuilt binary; otherwise `cargo build -p trioxide_cli` runs first.
// Set FIXTURE to a substring to run only matching fixtures. Island builds are cached in
// .out/.cache, so only changed islands invoke cargo.

import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { before, describe, test } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const fixtureRoots = ['tests/fixtures', 'tests/islands'].map((dir) => path.join(repo, dir));
// Inside this package so outputs resolve its node_modules (e.g. @oxc-project/runtime helpers).
const outDir = path.join(here, '.out');
const cacheDir = path.join(outDir, '.cache');

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

/** Runs trioxide and returns its stderr as well (island fallbacks are warnings). */
function runWithStderr(args) {
  const result = spawnSync(trioxide, args, { encoding: 'utf8' });
  assert.equal(result.status, 0, `trioxide ${args.join(' ')} failed:\n${result.stderr}`);
  return result.stderr;
}

/**
 * Imports `entry` in a fresh Node process, awaits its `main()` and returns stdout. `ready`
 * lists glue modules whose `ready()` must resolve first.
 */
function execute(entry, ready = []) {
  const waits = ready.map((glue) => `await (await import(${JSON.stringify(pathToFileURL(glue).href)})).ready();\n`);
  const script = `${waits.join('')}const m = await import(${JSON.stringify(pathToFileURL(entry).href)});
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

const entries = fixtureRoots
  .flatMap((root) => discover(root))
  .filter((entry) => !process.env.FIXTURE || entry.includes(process.env.FIXTURE));

/** Builds the fixture with its island and swaps the JS kernel for the island's glue. */
function buildWithIsland(fixture, entry, dir, glue) {
  prepare(dir);
  const spec = JSON.parse(readFileSync(path.join(fixture, 'island/island.json'), 'utf8'));
  const config = path.join(dir, '..', `trioxide-${glue}.json`);
  writeFileSync(config, JSON.stringify({ cacheDir, wasm: { glue } }));
  const stderr = runWithStderr([
    'build', '--emit=js', '--require-wasm', '--config', config, '--root', fixture,
    '--debug-island', path.join(fixture, 'island'), '--out-dir', dir, entry,
  ]);
  assert.doesNotMatch(stderr, /warning|error/, stderr);
  const gluePath = path.join(dir, '_islands', `${spec.id}.js`);
  assert.ok(existsSync(gluePath) && existsSync(path.join(dir, '_islands', `${spec.id}.wasm`)), 'island not emitted');
  renameSync(path.join(dir, 'kernel.js'), path.join(dir, 'kernel.fallback.js'));
  const names = spec.exports.map((e) => e.name).join(', ');
  writeFileSync(path.join(dir, 'kernel.js'), `export { ${names} } from "./_islands/${spec.id}.js";\n`);
  return gluePath;
}

for (const entry of entries) {
  const fixture = path.dirname(entry);
  const root = fixtureRoots.find((dir) => fixture.startsWith(dir + path.sep));
  const name = path.join(path.basename(root), path.relative(root, fixture));
  const work = path.join(outDir, name);
  const hasIsland = existsSync(path.join(fixture, 'island/island.json'));

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

    for (const glue of ['tla', 'ready']) {
      test(`c: trioxide JS+WASM (${glue} glue)`, (t) => {
        if (!hasIsland) return t.skip('no islands (hand-written ones live in tests/islands)');
        if (stripped?.status !== 0) return t.skip('baseline failed');
        const dir = path.join(work, `c-${glue}`);
        const gluePath = buildWithIsland(fixture, entry, dir, glue);
        const withWasm = execute(path.join(dir, jsName(path.basename(entry))), glue === 'ready' ? [gluePath] : []);
        assert.equal(withWasm.status, 0, `JS+WASM variant failed:\n${withWasm.stderr}`);
        assert.equal(withWasm.stdout, stripped.stdout);
      });
    }
  });
}
