# Benchmarks

`pnpm --filter trioxide-benchmarks bench` compiles every case in `cases/` twice (types
stripped only, and with trioxide's JS passes) and times both versions with tinybench, each in
a process of its own, with V8's defaults and with `--no-opt`. `bench.mjs` describes the
options (`TRIOXIDE_BIN`, `CASE`, `BENCH_TIME`).

| Case | Pass (PLAN §4) | Measures |
|------|----------------|----------|
| `inline-vector` | 5, function inlining | vector helpers and methods called in a loop |
| `scalar-points` | 8, scalar replacement | `{x, y}` points that never leave the loop |
| `partial-eval-table` | 11, partial evaluation | evaluating a module that builds lookup tables |
| `closure-hoist-loop` | 12, closure hoisting | closures that capture nothing, made per iteration |
