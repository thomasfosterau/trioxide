# Benchmarks

`pnpm --filter trioxide-benchmarks bench` compiles every case in `cases/` twice (types
stripped only, and with trioxide's JS passes) and times both versions with tinybench, each in
a process of its own, over several alternating rounds, with V8's defaults and cold
(`--no-opt --no-maglev`). A case's `trioxide.json` turns on the passes that are off by default.
`bench.mjs` describes the options (`TRIOXIDE_BIN`, `CASE`, `ROUNDS`, `BENCH_TIME`).

| Case | Pass (PLAN §4) | Measures |
|------|----------------|----------|
| `inline-vector` | 5, function inlining | vector helpers and methods called in a loop |
| `scalar-points` | 8, scalar replacement | `{x, y}` points that never leave the loop |
| `partial-eval-table` | 11, partial evaluation | evaluating a module that builds lookup tables |
| `closure-hoist-loop` | 12, closure hoisting | closures that capture nothing, made per iteration |
| `array-pipelines` | 6, array pipelines | `map`/`filter`/`reduce` chains and searches over arrays |
| `for-of-loops` | 7, `for…of` lowering | nested `for…of` over arrays, and array spreads |
| `licm-lengths` | 9, loop-invariant code motion | array lengths and pure math in loops that write other arrays |
| `hidden-class-objects` | 10, hidden-class stabilisation | instances that get properties late, sometimes, or lose them |
