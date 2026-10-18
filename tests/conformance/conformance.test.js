// Conformance smoke (docs/PLAN.md §8): real TypeScript libraries, vendored under
// tests/conformance/<lib>/, built by trioxide in several configurations and checked three ways:
//
//   1. every output module parses (`node --check`);
//   2. the library's own test suite passes against the output (vitest, which runs the
//      vendored mocha/jest/vitest tests unchanged with `globals: true`);
//   3. the library's conformance driver (`main.ts`, ours, deterministic) prints exactly what
//      the types-stripped build prints.
//
// Configurations: `stripped` (`trioxide strip`, the baseline), `default` (no config: `auto`
// mode, so islands the cost model picks are built and used), `trust-types` (`trustTypes: true`)
// and `all-passes` (every pass on, including the off-by-default `inline-fn` and `scalar`, and
// `specialize`). Every library module and the driver are entries: the tests import modules
// directly, so all of their exports are open.
//
// Slow-ish (a vitest run per library and configuration), so it only runs with
// TRIOXIDE_CONFORMANCE=1. TRIOXIDE_BIN picks a prebuilt binary (else `cargo build -p
// trioxide_cli` runs first); LIBRARY filters libraries by substring.

import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { before, describe, test } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const outDir = path.join(here, '.out');
const cacheDir = path.join(outDir, '.cache');
const enabled = process.env.TRIOXIDE_CONFORMANCE === '1';

/**
 * The vendored libraries: where their tests live (copied next to the output unchanged), which
 * test files vitest runs, other files the tests read, and tests that already fail on the
 * types-stripped build (they must fail the same way on every other build).
 */
export const LIBRARIES = {
  mitt: { tests: 'test', include: ['test/*_test.ts'] },
  'heap-js': {
    tests: 'tests',
    include: ['tests/**/*.test.ts'],
    // `HeapAsync#_topN_heap` pushes `undefined` once the heap runs out, which vitest's
    // `toEqual` rejects (upstream runs jest). Fails on the stripped build too.
    knownFailures: [/HeapAsync private heap types .* #_topHeapN\(N\) should return empty array when heap is empty$/],
  },
  neverthrow: {
    tests: 'tests',
    include: ['tests/*.test.ts'],
    // Upstream runs vitest 2; vitest 5 fails an un-awaited `expect().rejects` and compares
    // error classes in `toEqual`. Both fail on the stripped build too.
    knownFailures: [
      /ResultAsync acting as a Promise<Result> rejects if the underlying promise is rejected$/,
      /ResultAsync\.fromThrowable Accepts an error handler as a second argument$/,
    ],
  },
  'simplex-noise': { tests: 'test', include: ['test/simplex-noise-test.ts'], copy: ['snapshots'] },
};

export const CONFIGURATIONS = {
  default: {},
  'trust-types': { trustTypes: true },
  'all-passes': {
    passes: {
      inline: true, checks: true, fold: true, dce: true, licm: true, 'hidden-class': true, 'for-of': true,
      pipelines: true, 'partial-eval': true, scalar: true, 'closure-hoist': true, params: true,
      'inline-fn': true, simplify: true, imports: true, specialize: true,
    },
  },
};

let trioxide = process.env.TRIOXIDE_BIN;

before(() => {
  if (!enabled || trioxide) return;
  execFileSync('cargo', ['build', '--quiet', '-p', 'trioxide_cli'], { cwd: repo, stdio: 'inherit' });
  trioxide = path.join(repo, 'target/debug/trioxide');
});

/** The library's TypeScript modules (relative paths), without its tests and our driver. */
function sources(lib, dir = '') {
  const root = path.join(here, lib);
  const found = [];
  for (const name of readdirSync(path.join(root, dir)).sort()) {
    const relative = path.join(dir, name);
    if (statSync(path.join(root, relative)).isDirectory()) {
      if (relative !== LIBRARIES[lib].tests && !(LIBRARIES[lib].copy ?? []).includes(relative)) {
        found.push(...sources(lib, relative));
      }
    } else if (name.endsWith('.ts') && !name.endsWith('.d.ts') && relative !== 'main.ts') {
      found.push(relative);
    }
  }
  return found;
}

function prepare(dir) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
}

/** Every `.js` file under `dir` but the copied tests' and node_modules'. */
function outputs(dir) {
  return readdirSync(dir, { recursive: true })
    .filter((file) => file.endsWith('.js') && !file.includes('node_modules'))
    .map((file) => path.join(dir, file));
}

/** Builds `lib` into `dir` in one configuration; returns the island glue modules and stderr. */
function build(lib, name, dir) {
  if (name === 'stripped') rmSync(path.join(outDir, lib), { recursive: true, force: true });
  prepare(dir);
  if (name === 'stripped') {
    for (const file of [...sources(lib), 'main.ts']) {
      const target = path.join(dir, file.replace(/\.ts$/, '.js'));
      mkdirSync(path.dirname(target), { recursive: true });
      writeFileSync(target, execFileSync(trioxide, ['strip', '--root', path.join(here, lib), path.join(here, lib, file)], { encoding: 'utf8' }));
    }
    return { glues: [], stderr: '' };
  }
  // Not next to `dir`: `..` from the tests would resolve to `<name>.json` before `<name>/`.
  const config = path.join(outDir, lib, 'configs', `${name}.json`);
  mkdirSync(path.dirname(config), { recursive: true });
  writeFileSync(config, JSON.stringify({ ...CONFIGURATIONS[name], cacheDir }));
  const root = path.join(here, lib);
  const entries = [...sources(lib), 'main.ts'].map((file) => path.join(root, file));
  const result = spawnSync(trioxide, ['build', '--config', config, '--root', root, '--out-dir', dir, ...entries], {
    encoding: 'utf8',
  });
  assert.equal(result.status, 0, `trioxide build (${name}) failed:\n${result.stderr}`);
  const islands = path.join(dir, '_islands');
  const glues = existsSync(islands)
    ? readdirSync(islands).filter((file) => file.endsWith('.js')).map((file) => path.join(islands, file))
    : [];
  return { glues, stderr: result.stderr };
}

/** Copies the tests (and what they read) next to the output, with a package.json and vitest config. */
function addTests(lib, dir) {
  const { tests, include, copy = [] } = LIBRARIES[lib];
  for (const item of [tests, ...copy]) cpSync(path.join(here, lib, item), path.join(dir, item), { recursive: true });
  // `main` lets a test import the package root (mitt's `import mitt from '..'`).
  const main = sources(lib).find((file) => /(^|\/)index\.ts$/.test(file)) ?? sources(lib)[0];
  writeFileSync(path.join(dir, 'package.json'), JSON.stringify({ type: 'module', main: `./${main.replace(/\.ts$/, '.js')}` }));
  writeFileSync(
    path.join(dir, 'vitest.config.mjs'),
    `export default { test: { globals: true, include: ${JSON.stringify(include)}, watch: false, testTimeout: 30000 } };\n`,
  );
}

/** Runs the driver's `main()` once the islands are ready, with extensionless imports resolved. */
function runMain(dir, glues) {
  const waits = glues.map((glue) => `await (await import(${JSON.stringify(pathToFileURL(glue).href)})).ready();`).join(' ');
  const script = `const m = await import(${JSON.stringify(pathToFileURL(path.join(dir, 'main.js')).href)}); ${waits} await m.main();`;
  return spawnSync(process.execPath, ['--import', pathToFileURL(path.join(here, 'register.mjs')).href, '--input-type=module', '--eval', script], {
    encoding: 'utf8',
    cwd: dir,
    timeout: 60_000,
  });
}

/** Runs the library's own tests in `dir`; returns `fullName → status` and the run's output. */
function runTests(dir) {
  const report = path.join(dir, 'vitest.json');
  const result = spawnSync(
    process.execPath,
    [path.join(here, 'node_modules/vitest/vitest.mjs'), 'run', '--config', 'vitest.config.mjs', '--reporter=json', `--outputFile=${report}`],
    { encoding: 'utf8', cwd: dir, timeout: 300_000, env: { ...process.env, CI: '1', NO_COLOR: '1' } },
  );
  assert.ok(existsSync(report), `vitest wrote no report:\n${result.stdout}\n${result.stderr}`);
  const json = JSON.parse(readFileSync(report, 'utf8'));
  const statuses = new Map();
  for (const file of json.testResults) {
    if (file.status === 'failed' && file.assertionResults.length === 0) {
      statuses.set(`${path.relative(dir, file.name)} (suite)`, `failed: ${file.message}`);
    }
    for (const t of file.assertionResults) {
      statuses.set(`${path.relative(dir, file.name)} > ${t.fullName}`, t.status === 'failed' ? `failed: ${t.failureMessages.map((m) => m.split('\n')[0]).join('; ')}` : t.status);
    }
  }
  return statuses;
}

const libraries = Object.keys(LIBRARIES).filter((lib) => !process.env.LIBRARY || lib.includes(process.env.LIBRARY));

for (const lib of libraries) {
  describe(lib, { skip: enabled ? false : 'set TRIOXIDE_CONFORMANCE=1 to run' }, () => {
    let baseline;
    for (const name of ['stripped', ...Object.keys(CONFIGURATIONS)]) {
      test(name, (t) => {
        if (name !== 'stripped' && !baseline) return t.skip('the stripped build failed');
        const dir = path.join(outDir, lib, name);
        const { glues, stderr } = build(lib, name, dir);
        assert.doesNotMatch(stderr, /error/, stderr);
        if (stderr.trim()) t.diagnostic(stderr.trim());
        if (glues.length > 0) t.diagnostic(`islands: ${glues.map((glue) => path.basename(glue, '.js')).join(', ')}`);

        for (const file of outputs(dir)) {
          const check = spawnSync(process.execPath, ['--check', file], { encoding: 'utf8' });
          assert.equal(check.status, 0, `${path.relative(dir, file)} does not parse:\n${check.stderr}`);
        }

        addTests(lib, dir);
        const main = runMain(dir, glues);
        assert.equal(main.status, 0, `main() failed:\n${main.stderr}`);
        const tests = runTests(dir);
        assert.ok(tests.size > 0, 'no tests ran');
        t.diagnostic(`${tests.size} tests`);

        if (name === 'stripped') {
          const known = LIBRARIES[lib].knownFailures ?? [];
          const failed = [...tests].filter(
            ([test, status]) => status !== 'passed' && status !== 'skipped' && status !== 'pending' && !known.some((re) => re.test(test)),
          );
          assert.deepEqual(failed, [], 'the library\'s tests fail on the types-stripped build');
          baseline = { stdout: main.stdout, tests };
          return;
        }
        assert.equal(main.stdout, baseline.stdout, 'main() prints something else than the stripped build');
        const differences = [...baseline.tests]
          .filter(([test, status]) => tests.get(test) !== status)
          .map(([test, status]) => `${test}: ${status} → ${tests.get(test) ?? 'missing'}`);
        assert.deepEqual(differences, [], 'test results differ from the stripped build');
      });
    }
  });
}
