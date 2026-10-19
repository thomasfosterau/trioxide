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
6.18; Rust 1.99.1; the release `trioxide` binary of this branch at `70d687f` (main at `80ad92a`,
with #6 and #16, plus this PR, including the refitted cost model). Defaults throughout: five
rounds for `passes` and `programs`, three for `partition` and `throughput`. Run-to-run noise
on this machine is heavy-tailed: one process in five or so of the same build runs 20–30% slow,
so read the ranges, not just the medians, and prefer more rounds (`ROUNDS=11`) for a single
decision.

### JS passes

| case | engine | measures | stripped | optimised | speedup (range) |
|---|---:|---:|---:|---:|---:|
| `array-pipelines` | default | `bench()` | 1.34 ms | 38.6 µs | 33.90× (24.14–37.37) |
| `array-pipelines` | cold | `bench()` | 1.82 ms | 718.4 µs | 2.59× (2.39–2.62) |
| `closure-hoist-loop` | default | `bench()` | 1.1 µs | 2.0 µs | 0.60× (0.36–0.84) |
| `closure-hoist-loop` | cold | `bench()` | 8.9 µs | 10.3 µs | 0.86× (0.75–1.29) |
| `for-of-loops` | default | `bench()` | 42.8 µs | 36.0 µs | 1.19× (1.10–1.35) |
| `for-of-loops` | cold | `bench()` | 892.0 µs | 495.2 µs | 1.94× (1.52–3.20) |
| `hidden-class-objects` | default | `bench()` | 930.6 µs | 199.2 µs | 4.62× (4.30–6.25) |
| `hidden-class-objects` | cold | `bench()` | 2.80 ms | 1.86 ms | 1.57× (1.46–1.80) |
| `inline-vector` | default | `bench()` | 5.8 µs | 6.9 µs | 0.87× (0.78–1.06) |
| `inline-vector` | cold | `bench()` | 176.4 µs | 143.6 µs | 1.23× (1.19–1.26) |
| `licm-lengths` | default | `bench()` | 61.4 µs | 53.8 µs | 0.89× (0.69–1.36) |
| `licm-lengths` | cold | `bench()` | 1.02 ms | 686.5 µs | 1.52× (1.39–1.67) |
| `partial-eval-table` | default | module evaluation | 399.0 µs | 128.6 µs | 3.06× (2.16–3.36) |
| `partial-eval-table` | cold | module evaluation | 446.9 µs | 201.0 µs | 2.33× (1.85–2.56) |
| `scalar-instances` | default | `bench()` | 7.7 µs | 9.1 µs | 0.83× (0.83–0.88) |
| `scalar-instances` | cold | `bench()` | 867.3 µs | 239.1 µs | 3.63× (2.94–3.92) |
| `scalar-points` | default | `bench()` | 7.0 µs | 8.5 µs | 0.84× (0.70–0.87) |
| `scalar-points` | cold | `bench()` | 621.5 µs | 190.3 µs | 3.13× (3.08–4.41) |

- S5b's passes (#16) win big under V8's defaults where they apply: `array-pipelines` 34×,
  `hidden-class-objects` 4.6×, `for-of-loops` 1.2×; `licm-lengths` only cold.
- S5a's passes: partial evaluation wins everywhere; inlining and scalar replacement win only
  in cold code and are slightly slower hot (which is why they are off by default).
  `scalar-instances` (S10: `new Point(…)` temporaries) behaves like `scalar-points`: 0.83×
  hot, 3.6× cold (11 alternating rounds against main's build of the same case, which leaves
  the instances alone: 0.84× (0.76–0.86) and 3.62× (2.73–4.05)).
- `closure-hoist-loop` didn't measure closure hoisting: #16's pipelines pass turned its
  4-element `rows[i].map(…)` into a `push` loop first, and that loop is slower than V8's
  builtin `map` (0.60× hot, 0.86× cold). S10 leaves a chain that is one `map` as the builtin
  call (nothing to fuse): with the same five rounds, `closure-hoist-loop` went from 0.65×
  (0.62–0.66) to 1.02× (0.95–1.11) under V8's defaults and from 0.87× (0.79–0.89) to 1.52×
  (1.45–1.60) cold; every other case stayed within its range (`array-pipelines` 29.8× before
  and after), and so did the whole programs (`array-pipeline` 1.15× → 1.17×, `telemetry`
  1.13× → 1.13× under V8's defaults).

### Whole programs

| program | engine | stripped | trioxide | all passes | auto + WASM | islands |
|---|---:|---:|---:|---:|---:|---:|
| `array-pipeline` | default | 7.68 ms | 1.18× (0.81–1.30) | 1.21× (1.13–1.44) | 1.21× (1.15–1.41) | — |
| `array-pipeline` | cold | 11.64 ms | 1.01× (0.99–1.18) | 1.03× (0.98–1.20) | 1.00× (0.88–1.14) | — |
| `numeric-driver` | default | 2.16 ms | 1.01× (0.98–1.08) | 0.97× (0.97–1.06) | 1.16× (1.14–1.47) | dsp |
| `numeric-driver` | cold | 33.12 ms | 1.03× (0.95–1.33) | 1.01× (0.98–1.30) | 6.89× (6.03–8.97) | dsp |
| `object-heavy` | default | 1.56 ms | 1.00× (0.95–1.14) | 0.91× (0.78–1.15) | 1.03× (0.99–1.10) | — |
| `object-heavy` | cold | 26.07 ms | 1.08× (1.00–1.17) | 1.12× (0.99–1.34) | 1.06× (1.00–1.08) | — |
| `examples/image-filters` | default | 32.09 ms | 1.02× (0.94–1.89) | 1.02× (0.69–1.48) | 1.00× (0.99–1.11) | — |
| `examples/image-filters` | cold | 203.73 ms | 1.02× (0.91–1.03) | 1.18× (1.03–1.28) | 1.09× (0.92–1.28) | — |
| `examples/mixed` | default | 1.22 ms | 1.01× (0.88–1.04) | 1.02× (0.87–1.03) | 1.52× (1.23–1.64) | kernel |
| `examples/mixed` | cold | 47.80 ms | 1.01× (0.90–1.06) | 1.02× (0.91–1.05) | 6.66× (6.50–6.85) | kernel |
| `examples/telemetry` | default | 946.3 µs | 1.07× (1.05–1.13) | 1.08× (0.98–1.29) | 1.07× (1.02–1.11) | — |
| `examples/telemetry` | cold | 9.41 ms | 1.54× (1.45–1.78) | 1.59× (1.47–1.87) | 1.54× (1.53–1.64) | — |

(`examples/telemetry`'s own config already turns on `inline-fn` and `scalar`, so its
`trioxide` and `all passes` columns are the same build; its README compares against the
default passes.) The variants agree on every result in every round.

- **WASM pays off where the cost model picks it**: `mixed`'s int32 kernel (1.5× under V8's
  defaults, 6.7× cold) and `numeric-driver`'s FIR filter (`fir`, 1.16× and 6.9×). With the
  weights before the refit, `numeric-driver` stayed all JS and `image-filters` moved `boxBlur`
  into WASM, which is slower (0.79× over eleven rounds; its README has the numbers).
- **The JS passes now move `array-pipeline`**: 1.18× with the default passes and 1.21× with
  every pass under V8's defaults (every round above 1× with every pass on), from #16's
  pipeline lowering. Cold, it is a tie. `telemetry` gains 1.5× cold. `object-heavy` is within
  the noise either way.

### M4: "Benchmarks show speedups on array-pipeline and object-heavy fixtures"

**Half of it holds on main**, now that #16 has landed. The single-pass cases show it
clearly (`array-pipelines` 34× and `hidden-class-objects` 4.6× under V8's defaults). Of the
realistic programs, `array-pipeline` is 1.18–1.21× faster under V8's defaults (cold, a tie),
but `object-heavy` stays within the noise of the stripped build in both engines, with the
default passes and with every pass. What still limits them, from `trioxide explain`:

1. **`object-heavy`'s objects are class instances.** Its `Vec2` temporaries come from
   `new Vec2(…)`, which scalar replacement doesn't take (object literals only); hidden-class
   stabilisation doesn't fire on them. Method calls on member receivers (`p.velocity.add(…)`)
   aren't inlined (the receiver must be a never-reassigned name or `this`). `for…of` lowers
   its four loops, and with every pass on `inline-fn` and `scalar` change two calls and one
   literal; V8 already optimises the rest.
2. **`array-pipeline`'s chains are partly lowered.** `revenue`, `revenueByCategory`,
   `bigSpenders` and `quantityHistogram` each get a loop; `bigSpenders`'s
   `totals.map(…).filter(…).sort(…)` stays because `totals` (`new Array(500).fill(0)`) is only
   Declared, and `stats`' reductions aren't touched.
3. **Closure hoisting refuses capture-free callbacks outside loops in modules with a runtime
   import**, whether or not an import cycle exists: in `array-pipeline/report.ts` (which
   imports a constant from `orders.ts`, which imports nothing) most of them stay. Checking
   the analysis's actual import cycles would let them move (S5a).
4. **V8 already does much of the rest in hot code**, as the cold/default split of S5a's cases
   shows.

### Compiler throughput

`trioxide build` from a cold process, every module an entry, default options (each library's
modules and driver for the conformance corpus; the analysis bench's corpus of
`crates/trioxide_types/tests/corpus` and `tests/fixtures/analysis`, 20 copies, now 1940
modules), median of three runs:

| corpus | modules | wall time | peak RSS |
|---|---:|---:|---:|
| conformance/heap-js | 3 | 61.82 ms (61.00 ms–67.02 ms) | 37 MiB (37–37) |
| conformance/mitt | 2 | 47.71 ms (42.18 ms–53.69 ms) | 32 MiB (32–32) |
| conformance/neverthrow | 6 | 63.98 ms (63.45 ms–90.79 ms) | 40 MiB (40–40) |
| conformance/simplex-noise | 2 | 40.98 ms (39.17 ms–53.46 ms) | 33 MiB (33–33) |
| analysis bench corpus (1940 modules) | 1940 | 1.71 s (1.50 s–1.84 s) | 283 MiB (282–284) |

This is the baseline. A whole build of the 1940-module corpus (graph, tsrs program, analysis,
partitioning, passes and codegen of every module) takes about 1.7 s and 280 MiB with #6's
parallel analysis and #16's passes (before #6 it was 2.2 s and 220 MiB on the same corpus); S2's
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
- **S10 fixed the second and third in the lowering** (`ToInt32` of constants folded and of
  element reads taken from the element, `a[i]++`/`+=` on typed arrays in the element type's
  arithmetic). The partition suite, five rounds, WASM before → after (JS in brackets):
  `histogram` 3.45 ms → 191.5 µs (231.9 µs), `noise` 9.63 → 5.92 ms (8.00 ms), `int-hash`
  4.65 → 3.79 ms (10.19 ms), `per-element` 1.54 ms → 752.1 µs (185.7 µs); `histogram` and
  `noise` now run faster in WASM than in JS. The first miss is the analysis's (S10's
  integer-ranges change).
- The `fmod` paths cost nothing measurable: `floor % 2.0` only runs on exact `.5` values,
  and `to_uint32`'s `%` only out of the `u32` range. Not worth a change for speed.
- `rounding`: with its counter now int32 the old weights picked it (2.1× faster in WASM); the
  refitted ones don't, by 0.18 ms of predicted saving (one `math` weight for every `Math`
  call). Separating `round`/`floor`/`sign`/`trunc` from the transcendental functions would
  change the model's form, which S9 didn't (S8).
- Loop counters typed `f64`: fixed on main for counters bounded by parameters and `.length`
  (`for (let i = 0; i < n; i++)`), and in S10 for a counter stepped by more than one against a
  length (`for (let i = 0; i + 3 < a.length; i += 4)`, image-filters' `grayscale`).
- The same `boxBlur` source lowers its inner index in `i32` in `examples/image-filters` (the
  analysis proves `ny * width` int32 there) and in `f64` in the calibration kernel, whose
  driver passes the same constants. Why the products' ranges differ between the two programs
  is a question for S2. **Answered in S10:** node facts join every run and loop iteration of
  their unit, and in one iteration of the kernel's (the order of runs differs between the two
  programs) `ny` was exactly -1, so `ny >= 0` narrowed it to the inverted interval [0, -1]
  instead of `never`; arithmetic on that gave `ny * width` [-320, 0], and the product with 4
  then lost `nonNegZero`, which the backend needs for `i32`. S10 narrows a contradiction to
  `never`, keeps `nonNegZero` on products of integers that can't be -0, and narrows `i` from
  `i + 3 < a.length` (the step-4 counter of `grayscale`). Partition suite, five rounds:
  `image-filters`' island 22.46 → 6.26 ms (JS 4.99 ms), `per-element` 1.54 ms → 761.5 µs.
- Even with `i32` indices, `examples/image-filters`' `boxBlur` island is slower than JS
  (40.9 against 32.9 ms for the whole `run()`, the median of eleven rounds; removing the
  `black_box` on its divisor changes nothing), so what makes it slow isn't only the index
  type; it needs a profile. The refitted weights keep it in JS.
