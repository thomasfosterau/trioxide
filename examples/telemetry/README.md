# telemetry

A CLI-style workload for trioxide's JS speed passes: a gateway that verifies, decodes and
summarises one upload of sensor packets.

- `src/crc.ts`: CRC-32, a nibble at a time, with a 16-entry table built when the module loads.
- `src/readings.ts`: 8-byte packets decoded into reading records through small helpers
  (`clamp`, `toCelsius`).
- `src/stats.ts`: per-device summaries with a local accumulator object, alerts and a top-3,
  written as array pipelines with small callbacks.
- `src/main.ts`: `run()` does one upload's work; `main()` prints it.

`trioxide.json` turns on the two passes that are off by default (`inline-fn` and `scalar`). No
function here is a WASM candidate worth an island (strings, objects and arrays of records), so
the build is all JS.

```sh
pnpm start           # trioxide build src/main.ts --out-dir dist, then run main()
pnpm explain         # trioxide explain --entry src/main.ts
pnpm bench           # the benchmarks' whole-program suite for this example
```

## What the passes do

From `pnpm explain` and the output in `dist/`:

- **Partial evaluation (pass 11)**: `const NIBBLES = crcNibbleTable()` becomes the literal
  16-entry table, so loading the module runs no table code.
- **Scalar replacement (pass 8)**: `summarise`'s accumulator `{ count, sum, min, max,
  humidity }` becomes five locals (`acc_count`, `acc_sum`, …).
- **Inlining (pass 5)**: `toCelsius` and `clamp` are inlined into `decode`'s loop
  (`humidity: (value_1 = …, value_1 < 0 ? 0 : value_1 > 100 ? 100 : value_1)`).
- **Closure hoisting (pass 12)**: the callbacks of `alerts` and `hottest` that capture
  nothing (`(r) => \`${r.device}@${r.time}\``, the `sort` comparator, …) are created once, at
  the top of the module.
- **`for…of` lowering (pass 7)**: `summarise`'s `for (const r of readings)` becomes an indexed
  loop.
- **Pipelines (pass 6)**: `hottest`'s final `.map((s) => s.device)` becomes a loop (the
  `filter`, `sort` and `slice` before it stay builtin calls).
- **Loop-invariant code motion (pass 9)**: `crc32` reads `bytes.length` once.
- **Constant inlining and folding (passes 1, 2)**: `dewPoint`'s `a` and `b` constants.

What they don't do, and why (`explain` logs each skip):

- `alerts`' `filter` callback captures `limit`, so it is created per call.
- `const BATCH = packets(4000, 2026)` isn't evaluated at build time: `packets` calls a closure
  (`lcg`), which the partial evaluator doesn't model, and its 32 000 bytes would be far over
  the 128-value literal limit anyway.
- `crcNibbleTable` stays in the output after partial evaluation removed its only call (the
  foundation DCE runs before the speed passes).

## Before and after

`node ../../benchmarks/bench.mjs programs --filter telemetry`, and the same harness with the
default passes only: each variant in a process of its own, nine alternating rounds, the
median time of `run()` and the range of the per-round speedups (Node 22.22, Intel Xeon
2.1 GHz, 4 cores; main with S5b's passes, #16):

| build | V8 defaults | cold (`--no-opt --no-maglev`) | fresh module + one `run()` |
|---|---:|---:|---:|
| types stripped | 932 µs | 9.24 ms | 1.84 ms |
| trioxide, default passes | 1.05× (0.80–1.13) | 1.41× (1.34–1.75) | 1.04× (0.79–1.06) |
| trioxide, this config (+ `inline-fn`, `scalar`) | 1.08× (0.87–1.12) | 1.54× (1.49–1.58) | 1.05× (0.97–1.09) |

Cold code is where the passes pay here: 1.4× with the default passes and 1.5× with every
speed pass on, since #16's passes landed (before them it was 1.05×). Under
V8's defaults the gain is within this machine's noise: the work is dominated by allocating
reading records and by `sort` and string building, which the passes leave alone.

## Same behaviour either way

The differential suite runs this example's `main()` types-stripped, as trioxide's JS and in
`auto` mode (`tests/differential`, legs a, b and d); all print the same.
`examples/mixed/test/examples.test.js` checks that it computes the same through the Vite
plugin, in production (`build.ssr`) and in dev (`ssrLoadModule`).
