# Conformance smoke

PLAN §8's conformance smoke: real, dependency-free TypeScript libraries, vendored unchanged,
built by `trioxide build` and checked against their own test suites and against the
types-stripped build.

```sh
TRIOXIDE_CONFORMANCE=1 pnpm --filter trioxide-conformance test
TRIOXIDE_CONFORMANCE=1 LIBRARY=heap TRIOXIDE_BIN=target/release/trioxide pnpm --filter trioxide-conformance test
```

Without `TRIOXIDE_CONFORMANCE` the suite is skipped (it runs a vitest per library and
configuration, about 20 s with a release binary); CI runs it in its own step.

## What it checks

`conformance.test.js` builds each library four ways, with every library module and the
driver as entries (the tests import modules directly, so all their exports stay open):

| configuration | config |
|---|---|
| `stripped` | `trioxide strip`, the baseline |
| `default` | none: `auto` mode, islands the cost model picks are built and used |
| `trust-types` | `trustTypes: true` |
| `all-passes` | every pass on, including `inline-fn`, `scalar` and `specialize` |

For each build:

1. every output module parses (`node --check`);
2. the library's own tests pass (vitest 5 with `globals: true` runs the upstream mocha, jest
   and vitest suites unchanged), with the same result per test as on the stripped build;
3. `main.ts`, a deterministic driver written for this harness (not upstream code), prints
   exactly what the stripped build prints.

The vendored sources keep their extensionless relative imports (`./result`, `..`): vitest
resolves them, and `register.mjs` adds a Node resolve hook for the driver.

## Libraries

| library | kind | upstream commit | license | tests |
|---|---|---|---|---|
| [`mitt`](https://github.com/developit/mitt) | utility (event emitter, generics) | `6b41670` (3.0.1) | MIT | 19 (mocha + chai + sinon) |
| [`heap-js`](https://github.com/ignlg/heap-js) | data structure (binary heap, classes and generics, sync and async) | `41206f6` (2.7.1) | BSD-3-Clause | 765 (jest) |
| [`neverthrow`](https://github.com/supermacro/neverthrow) | classes and generics (`Result`, `ResultAsync`, a TS namespace, generators) | `5ef3a01` | MIT | 117 (vitest) |
| [`simplex-noise`](https://github.com/jwagner/simplex-noise.js) | numeric (typed arrays, `Math`, PNG snapshot tests) | `6bfff87` (4.0.3) | MIT | 33 (mocha + chai) |

Each directory holds the upstream `LICENSE`, sources and tests as they are at that commit.
Left out: build configs, docs, benchmarks, and type-only tests (`mitt/test/test-types-compilation.ts`,
`neverthrow/tests/typecheck-tests.ts`, which need `tsc` and `ts-toolbelt`). Each library has its
own `tsconfig.json` (ES2022, bundler resolution, strict) in place of upstream's, which
targets ES5/CommonJS for publishing.

**Tests that already fail on the stripped build** are listed in `knownFailures`, and must fail
the same way everywhere:
- heap-js: `HeapAsync#_topN_heap` on an empty heap returns `[undefined, …]`, which vitest's
  `toEqual` rejects against `[]` (4 tests; upstream runs jest);
- neverthrow: two tests written for vitest 2 (an un-awaited `expect().rejects`, and `toEqual`
  between an `Error` and a `TypeError`), which vitest 5 fails.

## Results

All four libraries build, parse, pass their tests and match the stripped build in all three
configurations, with no warnings. No library gets an island in `default` mode: simplex-noise's
kernels are closures returned by factories, which the partitioner doesn't consider, and the
others aren't numeric.

How much of each library's own code the passes rewrite (diff lines, removed plus added,
against the stripped build; the driver and sourcemap comments excluded):

| library | default | trust-types | all-passes |
|---|---:|---:|---:|
| mitt | 0 | 0 | 0 |
| heap-js | 4 | 4 | 22 |
| neverthrow | 0 | 0 | 0 |
| simplex-noise | 13 | 13 | 13 |

Library code is mostly open (exported functions and classes the host calls with anything), so
the passes find little to prove: folded constants (`1 / 3`, `tableSize / 2`, `list[idx]` right
after `let idx = 0`) and, with `specialize`, heap-js's `repeat(str, times)` losing the `" "`
every caller passes. `trustTypes` changes nothing here; neither do `inline-fn` and `scalar`.
