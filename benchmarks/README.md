# Benchmarks

One package, `trioxide-benchmarks`, with one entry point and one timing harness:

```sh
cargo build -p trioxide_cli --release            # optional: bench builds it if stale
pnpm --filter trioxide-benchmarks bench          # every suite; writes .out/results.md (+ .json)
pnpm --filter trioxide-benchmarks bench programs throughput   # some suites
pnpm --filter trioxide-benchmarks bench --filter telemetry    # matching cases only
pnpm --filter trioxide-benchmarks bench --quick  # one short round, to check it runs (keeps results.json)
pnpm --filter trioxide-benchmarks fit            # refit the cost model (partition/fit.mjs)
pnpm --filter trioxide-benchmarks reach          # default-mode reach (partition/reach.mjs)
```

`TRIOXIDE_BIN` picks the binary (default: the release build, brought up to date first),
`ROUNDS` the rounds (default 5; 3 for partition and throughput), `BENCH_TIME` the milliseconds
each timing process runs (default 1000), and `BENCH_OUT` the report's path.

| suite | what | variants |
|---|---|---|
| `passes` | one case per JS speed pass (`passes/cases/`, see `passes/README.md`) | stripped, optimised |
| `programs` | whole programs: every example with a `run()` export, and `programs/` | stripped, trioxide, all passes, auto + WASM |
| `partition` | cost-model calibration kernels (`partition/`, see `partition/README.md`) | JS only, every cluster as WASM |
| `throughput` | `trioxide build` wall time and peak memory | — |

**The harness** (`lib/harness.mjs`, `lib/worker.mjs`): every variant runs in Node processes of
its own, so one variant's JIT state can't affect another's. Each round runs every variant
once; the order rotates by one each round and reverses every other cycle, so each variant
takes every position (two variants: first, second, second, first, …). A process warms up for a
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
6.18; Rust 1.99.1; the release `trioxide` binary of this branch (main at `baa82da`, with #6,
plus this PR, including the refitted cost model). Defaults throughout: five rounds for
`passes` and `programs`, three for `partition` and `throughput`. Run-to-run noise on this
machine is heavy-tailed: one process in five or so of the same build runs 20–30% slow, so read
the ranges, not just the medians, and prefer more rounds (`ROUNDS=11`) for a single decision.

### JS passes

| case | engine | measures | stripped | optimised | speedup (range) |
|---|---:|---:|---:|---:|---:|
| `closure-hoist-loop` | default | `bench()` | 1.1 µs | 1.1 µs | 1.06× (0.93–1.29) |
| `closure-hoist-loop` | cold | `bench()` | 8.5 µs | 6.1 µs | 1.39× (1.22–1.74) |
| `inline-vector` | default | `bench()` | 5.8 µs | 6.9 µs | 0.85× (0.68–0.86) |
| `inline-vector` | cold | `bench()` | 177.5 µs | 148.2 µs | 1.19× (1.07–1.45) |
| `partial-eval-table` | default | module evaluation | 433.4 µs | 148.9 µs | 2.62× (2.24–3.44) |
| `partial-eval-table` | cold | module evaluation | 517.8 µs | 202.5 µs | 2.20× (1.52–2.63) |
| `scalar-points` | default | `bench()` | 7.0 µs | 8.3 µs | 0.84× (0.83–0.90) |
| `scalar-points` | cold | `bench()` | 569.5 µs | 196.3 µs | 2.88× (2.52–3.32) |

The same picture as S5a measured with tinybench: partial evaluation wins everywhere, closure
hoisting, inlining and scalar replacement win only in cold code, and inlining and scalar
replacement are slightly slower in hot code (which is why they are off by default).

### Whole programs

| program | engine | stripped | trioxide | all passes | auto + WASM | islands |
|---|---:|---:|---:|---:|---:|---:|
| `array-pipeline` | default | 7.65 ms | 0.95× (0.85–1.04) | 0.96× (0.90–1.13) | 0.95× (0.85–0.99) | — |
| `array-pipeline` | cold | 11.96 ms | 1.00× (0.95–1.06) | 1.03× (1.03–1.10) | 1.04× (0.74–1.10) | — |
| `numeric-driver` | default | 2.18 ms | 0.99× (0.98–1.03) | 1.00× (0.66–1.07) | 1.15× (1.11–1.21) | dsp |
| `numeric-driver` | cold | 33.93 ms | 1.00× (0.73–1.09) | 1.04× (1.03–1.27) | 6.22× (5.00–7.79) | dsp |
| `object-heavy` | default | 1.58 ms | 1.04× (0.91–1.12) | 0.95× (0.93–1.03) | 0.95× (0.86–1.01) | — |
| `object-heavy` | cold | 25.96 ms | 0.98× (0.89–0.99) | 1.08× (0.98–1.17) | 1.00× (0.72–1.01) | — |
| `examples/image-filters` | default | 31.40 ms | 0.87× (0.74–1.02) | 0.98× (0.93–1.01) | 0.99× (0.97–1.02) | — |
| `examples/image-filters` | cold | 205.29 ms | 0.94× (0.93–1.11) | 0.97× (0.93–1.09) | 0.98× (0.90–1.16) | — |
| `examples/mixed` | default | 1.25 ms | 1.08× (1.02–1.50) | 1.06× (1.00–1.26) | 1.54× (1.49–1.83) | kernel |
| `examples/mixed` | cold | 50.34 ms | 1.19× (0.85–1.35) | 0.97× (0.81–1.10) | 6.56× (6.06–7.03) | kernel |
| `examples/telemetry` | default | 903.4 µs | 1.07× (0.86–1.11) | 1.05× (0.91–1.08) | 1.01× (0.84–1.10) | — |
| `examples/telemetry` | cold | 9.65 ms | 1.02× (0.91–1.14) | 1.08× (0.99–1.14) | 1.04× (0.97–1.16) | — |

(`examples/telemetry`'s own config already turns on `inline-fn` and `scalar`, so its
`trioxide` and `all passes` columns are the same build; its README compares against the
default passes. `examples/image-filters`' `trioxide` column is the same code as its stripped
build but for inlined constants, so its 0.87× is this machine's noise.) The variants agree on
every result in every round.

- **WASM pays off where the cost model now picks it**: `mixed`'s int32 kernel (1.5× under
  V8's defaults, 6.6× cold) and `numeric-driver`'s FIR filter (`fir`, 1.15× and 6.2×). With
  the weights before the refit, `numeric-driver` stayed all JS and `image-filters` moved
  `boxBlur` into WASM, which is slower (0.79× over eleven rounds; its README has the numbers).
- **The JS passes don't move these programs** beyond the noise, under either engine. The
  largest effect is `object-heavy` cold with every pass, 1.08×.

### M4: "Benchmarks show speedups on array-pipeline and object-heavy fixtures"

**It does not hold on main today**, for realistic programs. It holds for the synthetic
single-pass cases (`scalar-points` 2.9× and `inline-vector` 1.2× cold, `partial-eval-table`
2.2–2.6× everywhere), but `array-pipeline` and `object-heavy` run within ±8% of the stripped
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
   reach the same code; the gains are in cold code (`closure-hoist-loop` 1.39×,
   `scalar-points` 2.9× cold, both neutral or slower hot).

### Compiler throughput

`trioxide build` from a cold process, every module an entry, default options (each library's
modules and driver for the conformance corpus; the analysis bench's corpus of
`crates/trioxide_types/tests/corpus` and `tests/fixtures/analysis`, 20 copies, now 1940
modules), median of three runs:

| corpus | modules | wall time | peak RSS |
|---|---:|---:|---:|
| conformance/heap-js | 3 | 57.01 ms (56.66 ms–79.12 ms) | 37 MiB (37–37) |
| conformance/mitt | 2 | 40.01 ms (38.20 ms–48.11 ms) | 32 MiB (32–32) |
| conformance/neverthrow | 6 | 59.93 ms (59.52 ms–71.19 ms) | 40 MiB (40–40) |
| conformance/simplex-noise | 2 | 44.23 ms (43.06 ms–44.39 ms) | 33 MiB (33–33) |
| analysis bench corpus (1940 modules) | 1940 | 1.56 s (1.19 s–1.98 s) | 282 MiB (279–283) |

This is the baseline. A whole build of the 1940-module corpus (graph, tsrs program, analysis,
partitioning, passes and codegen of every module) takes about 1.6 s and 280 MiB with #6's
parallel analysis (before #6 it was 2.2 s and 220 MiB on the same corpus); S2's
analysis-only benchmark measured 770–800 ms of sequential analysis on 1820 of these modules.
The conformance libraries build in 40–60 ms each, most of it fixed start-up (loading tsrs's
`lib.d.ts`).

## Cost-model refit

S8 deferred the refit to this session; the details and tables are in `partition/README.md`.

- **Rerun on main**, the calibration kernels moved: the analysis now proves loop counters
  bounded by parameters and `.length` int32. `rounding` (S8's only miss) became a pick;
  `matmul` became a WASM win the old weights missed; `noise` became an old-weights pick that
  is still slower. The old weights agreed on 15 of 17.
- **Two metrics fixes first** (S8's `trioxide_partition::metrics`, see the PR): the model's
  idea of which values the island holds as integers now follows the backend's typing
  structurally. An element access used to count as int32-indexed whenever its index's range
  was int32, but the backend computes `+ - *` in `i32` only when both operands are integers
  and the node's range is proven, and `%`, `/` and `Math` calls in `f64` whatever their range.
  `image-filters`' kernel reads with `(ny * width + nx) * 4 + c`, and `a[(i % 7) * 4 + 1]` is
  float-indexed; the model counted both as int32 accesses.
  `crates/trioxide/tests/cost_metrics.rs` covers both.
- **The refit is applied** (`CostModel::default`, the schema and `options.d.ts`). The fitted
  weights pick the faster side on 16 of 17 kernels, against 15, and on the whole programs
  they move `examples/image-filters`' `boxBlur` back to JS (measured 0.79× in WASM) and
  `numeric-driver`'s `fir` into WASM (measured 1.15× faster), and keep `mixed`'s kernel. Their
  one kernel miss, `rounding` (2.2 ms per run left on the table), is the `Math.round`/`floor`/
  `sign` speed-up that one `math` weight can't separate from the slower transcendental
  functions. The values are kept as fitted, to three significant figures: `integer` and
  `iteration` are collinear, and rounding further flips `tile-digest` and `mixed`.
- What the model still can't see is in the generated code, below; refit after S6 fixes it.

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
- `rounding`: with its counter now int32 the old weights picked it (2.1× faster in WASM); the
  refitted ones don't, by 0.18 ms of predicted saving (one `math` weight for every `Math`
  call). Separating `round`/`floor`/`sign`/`trunc` from the transcendental functions would
  change the model's form, which S9 didn't (S8).
- Loop counters typed `f64`: fixed on main for counters bounded by parameters and `.length`
  (`for (let i = 0; i < n; i++)`); a counter stepped by more than one against a length
  (`for (let i = 0; i + 3 < a.length; i += 4)`, image-filters' `grayscale`) is still `f64`.
- The same `boxBlur` source lowers its inner index in `i32` in `examples/image-filters` (the
  analysis proves `ny * width` int32 there) and in `f64` in the calibration kernel, whose
  driver passes the same constants. Why the products' ranges differ between the two programs
  is a question for S2.
- Even with `i32` indices, `examples/image-filters`' `boxBlur` island is slower than JS
  (40.9 against 32.9 ms for the whole `run()`, the median of eleven rounds; removing the
  `black_box` on its divisor changes nothing), so what makes it slow isn't only the index
  type; it needs a profile. The refitted weights keep it in JS.
