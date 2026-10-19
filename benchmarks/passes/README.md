# Pass benchmarks

One case per JS speed pass. `pnpm --filter trioxide-benchmarks bench passes` (the `passes`
suite, `../suites/passes.mjs`) compiles every case in `cases/` twice, types stripped only and
with trioxide's JS passes (`--no-wasm`, with the case's `trioxide.json`, which turns on the
passes that are off by default), and times both with the shared harness (`../lib/harness.mjs`:
processes of their own, alternating rounds, median and range), under V8's defaults and cold
(`--no-opt --no-maglev`). `../README.md` has the numbers.

| Case | Pass (PLAN §4) | Measures |
|------|----------------|----------|
| `inline-vector` | 5, function inlining | vector helpers and methods called in a loop |
| `scalar-points` | 8, scalar replacement | `{x, y}` points that never leave the loop |
| `scalar-instances` | 8, scalar replacement | `new Point(x, y)` instances that never leave the loop |
| `partial-eval-table` | 11, partial evaluation | evaluating a module that builds lookup tables |
| `closure-hoist-loop` | 12, closure hoisting | closures that capture nothing, made per iteration |
| `array-pipelines` | 6, array pipelines | `map`/`filter`/`reduce` chains and searches over arrays |
| `for-of-loops` | 7, `for…of` lowering | nested `for…of` over arrays, and array spreads |
| `licm-lengths` | 9, loop-invariant code motion | array lengths and pure math in loops that write other arrays |
| `hidden-class-objects` | 10, hidden-class stabilisation | instances that get properties late, sometimes, or lose them |

A case exports `bench()`; one that also exports `measure = "init"` is timed by evaluating the
module afresh (which includes reading and parsing it).
