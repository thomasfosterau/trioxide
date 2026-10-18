# Benchmarks

One package, `trioxide-benchmarks`, with one entry point and one timing harness:

```sh
cargo build -p trioxide_cli --release            # optional; else target/debug, else built
pnpm --filter trioxide-benchmarks bench          # every suite; writes .out/results.md (+ .json)
pnpm --filter trioxide-benchmarks bench programs throughput   # some suites
pnpm --filter trioxide-benchmarks bench --filter telemetry    # matching cases only
pnpm --filter trioxide-benchmarks bench --quick  # one short round, to check it runs
pnpm --filter trioxide-benchmarks fit            # refit the cost model (partition/fit.mjs)
pnpm --filter trioxide-benchmarks reach          # default-mode reach (partition/reach.mjs)
```

`TRIOXIDE_BIN` picks the binary, `ROUNDS` the rounds (default 5; 3 for partition and
throughput), `BENCH_TIME` the milliseconds each timing process runs (default 1000), and
`BENCH_OUT` the report's path.

| suite | what | variants |
|---|---|---|
| `passes` | one case per JS speed pass (`passes/cases/`, see `passes/README.md`) | stripped, optimised |
| `programs` | whole programs: every example with a `run()` export, and `programs/` | stripped, trioxide, all passes, auto + WASM |
| `partition` | cost-model calibration kernels (`partition/`, see `partition/README.md`) | JS only, every cluster as WASM |
| `throughput` | `trioxide build` wall time and peak memory | — |

**The harness** (`lib/harness.mjs`, `lib/worker.mjs`): every variant runs in Node processes of
its own, so one variant's JIT state can't affect another's. Each round runs every variant
once, in an order that rotates and reverses from round to round. A process warms up for a
quarter of its time, then times batches of at least 10 ms and reports the median batch. The
report gives each variant's median time over the rounds and, against the first variant, the
median and the range of the per-round speedups; every round also checks that all variants
compute the same result. Runtime suites run under V8's defaults and **cold**
(`--no-opt --no-maglev`: interpreter and baseline compiler, how code runs before it is hot or
when it never gets hot); the partition suite uses the defaults only, since the cost model is
about steady state.

**Programs.** `trioxide` is the default options without WASM (`--no-wasm`, the program's own
`trioxide.json` applying), `all passes` adds `inline-fn` and `scalar`, and `auto + WASM` is the
default options (`mode: "auto"`) with the islands the cost model picks, built and ready.

- `array-pipeline`: reporting over 20 000 order records: `filter`/`map`/`reduce` chains,
  `forEach`, `sort`, `slice`, small callbacks.
- `object-heavy`: 2000 particles for 20 steps with an immutable `Vec2` class (a temporary per
  operation), class methods and object literals in the loop.
- `numeric-driver`: a JS driver feeding typed-array kernels frame by frame (a 63-tap FIR
  filter, RMS, quantisation, an integer checksum over 8 × 2048 samples).
- the examples `image-filters`, `mixed` and `telemetry` (`examples/`).

## Results

Node 22.22.0 (V8 12.4.254.21), Intel Xeon @ 2.10 GHz × 4 (a shared cloud VM), 16 GiB, Linux
6.18; Rust 1.99.1; the release `trioxide` binary at `9a24eb9` (main after #15–#19, plus this
branch's fixes). Run-to-run noise on this machine is ±10–30% for short programs, so read the
ranges, not just the medians.

### JS passes

| case | engine | measures | stripped | optimised | speedup (range) |
|---|---:|---:|---:|---:|---:|
| `closure-hoist-loop` | default | `bench()` | 1.3 µs | 1.1 µs | 1.06× (0.76–1.41) |
| `closure-hoist-loop` | cold | `bench()` | 9.3 µs | 6.2 µs | 1.46× (1.43–1.54) |
| `inline-vector` | default | `bench()` | 5.8 µs | 7.0 µs | 0.83× (0.82–0.96) |
| `inline-vector` | cold | `bench()` | 180.3 µs | 152.9 µs | 1.18× (0.90–1.25) |
| `partial-eval-table` | default | module evaluation | 431.4 µs | 156.4 µs | 2.76× (2.37–3.11) |
| `partial-eval-table` | cold | module evaluation | 501.6 µs | 240.5 µs | 2.06× (1.76–2.42) |
| `scalar-points` | default | `bench()` | 7.3 µs | 8.5 µs | 0.85× (0.83–1.14) |
| `scalar-points` | cold | `bench()` | 605.5 µs | 198.2 µs | 3.07× (3.05–3.36) |

The same picture as S5a measured with tinybench: partial evaluation wins everywhere, closure
hoisting, inlining and scalar replacement win only in cold code, and inlining and scalar
replacement are slightly slower in hot code (which is why they are off by default).

### Whole programs

| program | engine | stripped | trioxide | all passes | auto + WASM | islands |
|---|---:|---:|---:|---:|---:|---:|
| `array-pipeline` | default | 8.20 ms | 0.97× (0.93–1.02) | 1.00× (0.92–1.03) | 1.01× (0.96–1.06) | — |
| `array-pipeline` | cold | 12.32 ms | 0.94× (0.65–0.96) | 1.01× (0.95–1.21) | 0.94× (0.86–1.04) | — |
| `numeric-driver` | default | 2.33 ms | 0.99× (0.95–1.36) | 1.02× (0.91–1.35) | 1.00× (0.80–1.17) | — |
| `numeric-driver` | cold | 36.90 ms | 1.09× (0.90–1.59) | 1.00× (0.90–1.62) | 1.04× (0.92–1.65) | — |
| `object-heavy` | default | 2.29 ms | 1.01× (0.97–1.15) | 1.05× (0.92–1.11) | 0.91× (0.77–1.10) | — |
| `object-heavy` | cold | 27.37 ms | 0.97× (0.83–0.99) | 1.07× (1.02–1.25) | 0.94× (0.83–1.11) | — |
| `examples/image-filters` | default | 32.81 ms | 1.01× (0.94–1.24) | 0.98× (0.76–1.26) | 0.78× (0.77–1.01) | filters |
| `examples/image-filters` | cold | 223.24 ms | 1.02× (0.96–1.10) | 1.01× (0.86–1.14) | 1.57× (1.42–2.10) | filters |
| `examples/mixed` | default | 1.23 ms | 1.03× (0.92–1.18) | 1.00× (0.70–1.19) | 1.48× (1.40–1.61) | kernel |
| `examples/mixed` | cold | 53.58 ms | 1.01× (0.94–1.10) | 0.96× (0.82–1.09) | 5.81× (5.26–7.38) | kernel |
| `examples/telemetry` | default | 989.2 µs | 1.07× (1.05–1.58) | 1.09× (1.05–1.58) | 1.01× (0.83–1.38) | — |
| `examples/telemetry` | cold | 10.19 ms | 1.05× (0.98–1.07) | 1.03× (0.98–1.13) | 0.95× (0.85–1.05) | — |

(`examples/telemetry`'s own config already turns on `inline-fn` and `scalar`, so its
`trioxide` and `all passes` columns are the same build; its README compares against the
default passes.) The variants agree on every result in every round.

- **WASM pays off where the cost model says it does**: `mixed`'s int32 kernel, 1.5× under V8's
  defaults and 5.8× cold. It loses where the model is wrong: `image-filters`' `boxBlur`, which
  the model now picks (see "Cost-model refit"), is 0.78× under the defaults. (Cold, it is 1.6×,
  since the JS it replaces runs in the interpreter.) `numeric-driver`'s four kernels are all
  eligible by default and the model keeps them JS, but with every cluster as WASM
  (`node partition/compare.mjs input.ts` in `programs/numeric-driver`) `run()` takes 1.99 ms
  against 2.39 ms JS-only: a miss of the same kind as `matmul` (int32-indexed typed-array
  work, which the `index` weight still charges 2.5 ns per access).
- **The JS passes don't move these programs** beyond the noise, under either engine. The
  largest effect is `object-heavy` cold with every pass, 1.07×.

### M4: "Benchmarks show speedups on array-pipeline and object-heavy fixtures"

**It does not hold on main today**, for realistic programs. It holds for the synthetic
single-pass cases (`scalar-points` 3.1× and `inline-vector` 1.2× cold, `partial-eval-table`
2–2.8× everywhere), but `array-pipeline` and `object-heavy` run within ±7% of the stripped
build in both engines, with the default passes and with every pass. What blocks it, from
`trioxide explain` on those programs:

1. **Pipeline fusion and `for…of` lowering aren't on main.** They are PLAN §4 passes 6 and 7,
   S5b's #16, which is still in review; its own cases measure 35× (`array-pipelines`) and
   6.5× (`hidden-class-objects`) under V8's defaults. `array-pipeline` is exactly the code
   they target: none of its chains changes today. This suite picks up #16's cases and passes
   automatically once it merges; rerun it then.
2. **Closure hoisting refuses most callbacks in real modules.** In `array-pipeline/report.ts`
   every capture-free callback is skipped with "it isn't in a loop, and a module-level
   constant could be read before it exists through an import cycle": a module that imports
   anything may only hoist into a loop's enclosing function, whether or not an import cycle
   exists. (Here `report.ts` imports a constant from `orders.ts`, which imports nothing.)
   Checking the analysis's actual import cycles would let them move (S5a).
3. **Inlining and scalar replacement are off by default, and do little to class-based code.**
   In `object-heavy` the `Vec2` temporaries are class instances (`new Vec2(…)`), which scalar
   replacement doesn't take (object literals only), and method calls on member receivers
   (`p.velocity.add(…)`) aren't inlined (the receiver must be a never-reassigned name or
   `this`). With both passes on, one call in `step` and one call and one literal in `bounce`
   change.
4. **V8 already does the rest in hot code.** Where the passes do fire, the optimising tiers
   reach the same code; the gains are in cold code (`closure-hoist-loop` 1.46×,
   `scalar-points` 3.1× cold, both neutral or slower hot).

### Compiler throughput

`trioxide build` from a cold process, every module an entry, default options (each library's
modules and driver for the conformance corpus; the analysis bench's corpus of
`crates/trioxide_types/tests/corpus` and `tests/fixtures/analysis`, 20 copies, now 1940
modules), median of three runs:

| corpus | modules | wall time | peak RSS |
|---|---:|---:|---:|
| conformance/heap-js | 3 | 76.07 ms (68.47 ms–84.25 ms) | 35 MiB (35–35) |
| conformance/mitt | 2 | 39.89 ms (38.55 ms–49.04 ms) | 32 MiB (32–32) |
| conformance/neverthrow | 6 | 66.64 ms (60.11 ms–76.01 ms) | 39 MiB (39–39) |
| conformance/simplex-noise | 2 | 51.88 ms (51.25 ms–62.54 ms) | 33 MiB (33–33) |
| analysis bench corpus (1940 modules) | 1940 | 2.21 s (1.81 s–2.81 s) | 220 MiB (220–221) |

This is the baseline. A whole build of the 1940-module corpus (graph, tsrs program, analysis,
partitioning, passes and codegen of every module) takes about 2.2 s and 220 MiB; S2's
analysis-only benchmark measured 770–800 ms of analysis on 1820 of these modules. The
conformance libraries build in 40–80 ms each, most of it fixed start-up (loading tsrs's
`lib.d.ts`).

## Cost-model refit

S8 deferred the refit to this session; the details and tables are in `partition/README.md`.

- **Rerun on main**, the calibration kernels moved: the analysis now proves loop counters
  bounded by parameters and `.length` int32. `rounding` (S8's only miss) is now picked;
  `matmul` became a WASM win the model misses; `noise` became a model pick that is still
  slower. 15 of 17 agree with the current defaults.
- **One metrics bug fixed** (S8's `trioxide_partition::metrics`, see the PR): an element access
  counted as int32-indexed when its index expression's range was int32, but the backend
  computes `+ - *` in `i32` only when every operand is `i32` too. `image-filters`' kernel
  reads with `(ny * width + nx) * 4 + c`, which the island computes in `f64`; the model
  counted it as an int32 access. `crates/trioxide/tests/cost_metrics.rs` covers it.
- **The refit is rejected.** The fit also agrees on 15 of 17, and its misses lose more time
  per run (3.59 ms against the defaults' 1.61 ms; `node partition/fit.mjs`, which now decides
  like the model, instantiation included, and reports the time wrong picks lose). It doesn't
  fix `examples/image-filters` either: with the fitted weights `boxBlur` is still a WASM pick.
  Nor `numeric-driver`, which both weight sets keep JS though it is 1.2× faster in WASM.
  What the model misses is in the generated code, below; refit after S6 fixes it.

### For S6 and S7: what each known miss costs

Measured by editing the generated crate by hand, building it as the island builder does
(`opt-level = 3`, fat LTO, one codegen unit) and timing it in place of the generated one with
the harness (V8 defaults, three rounds). None of these edits is committed, and none is a
proposed fix as written (an `i32` index needs a range proof).

| miss (owner) | kernel | island as generated | island with the fix | JS |
|---|---|---:|---:|---:|
| index arithmetic in `f64` when a product's range isn't proven, and a counter stepped by 4 kept `f64` (S6) | image-filters | 23.3 ms | 6.5 ms | 8.0 ms |
| typed-array read-modify-write through `f64`: `bins[x]++` on a `Uint32Array` is `get` → `f64` → `+ 1` → `ToUint32` → `set` (S6, S7's `array` helpers) | histogram | 3.46 ms | 0.21 ms | 0.26 ms |
| `Math.imul(h, 0x9e3779b1)`: a literal above 2³¹ makes the call `math::imul(f64, f64)`, two run-time `ToInt32`s per call; folding it to `imul_i32(h, 0x9e3779b1_u32 as i32)` (S6) | noise | 9.90 ms | 6.25 ms | 8.26 ms |
| `fmod` in `to_uint8_clamp` (`floor % 2.0`) and `to_uint32` (out-of-range path) (S7) | image-filters, histogram, noise | 24.4 / 3.56 / 9.60 ms | 24.8 / 3.58 / 9.90 ms | — |
| `rounding` mispredicted (S8's note) | rounding | — | — | — |

- The first three each turn a kernel the island loses (1.1–14×) into one it wins
  (1.13–1.3×). Fixing them changes which operation kinds are slow, so the weights need a
  refit afterwards.
- The `fmod` paths cost nothing measurable: `floor % 2.0` only runs on exact `.5` values,
  and `to_uint32`'s `%` only out of the `u32` range. Not worth a change for speed.
- `rounding` is no longer mispredicted: with its counter now int32, the model picks it, and it
  is 2.1× faster in WASM.
- Loop counters typed `f64`: fixed on main for counters bounded by parameters and `.length`
  (`for (let i = 0; i < n; i++)`); a counter stepped by more than one against a length
  (`for (let i = 0; i + 3 < a.length; i += 4)`, image-filters' `grayscale`) is still `f64`.
- The same `boxBlur` source lowers its inner index in `i32` in `examples/image-filters` (the
  analysis proves `ny * width` int32 there) and in `f64` in the calibration kernel, whose
  driver passes the same constants. Why the products' ranges differ between the two programs
  is a question for S2.
- Even with `i32` indices, `examples/image-filters`' `boxBlur` island is slower than JS
  (40.6 against 32.1 ms for the whole `run()`; removing the `black_box` on its divisor changes
  nothing), so the example's misprediction isn't only the index type; it needs a profile.
