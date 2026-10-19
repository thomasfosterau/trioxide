# Partition benchmarks

Calibration data for the partitioner's cost model (`docs/PLAN.md` §5). Each kernel in
`kernels/<name>/` is a `kernel.ts` and an `input.ts` whose exported `run` (an arrow, so it
never joins the kernel's island) calls the kernel through the glue and returns a checksum
(and whose `main()` prints it). The host calling an export of the entry module runs its JS
version, so the work under test must sit behind a call inside the program. The kernels cover
a range of compute/boundary ratios: scalar kernels called once (float, int32, wrapping and
`Math` work), the same kind of arithmetic called per element from a JS loop, typed-array
image filters, a `number[]` reduction, small helpers.

```sh
cargo build -p trioxide_cli --release   # optional: the scripts build it if stale (or set TRIOXIDE_BIN)
node run.mjs              # time JS vs WASM, ask the cost model, print the confusion table
node fit.mjs              # least-squares fit of the weights to results.json
node fit.mjs --weights w.json   # evaluate other weights on the same measurements
node reach.mjs            # what qualifies with and without trustTypes, and what blocks it
node compare.mjs src/main.ts    # from a project: JS-only vs auto vs every cluster as WASM
```

`pnpm --filter trioxide-benchmarks bench` runs `run.mjs` as its `partition` suite.

`run.mjs` builds every kernel twice: JS-only (`mode: "js-only"`), and with every eligible
cluster as an island (`wasm-preferred` with no threshold), so the islands are exactly the
ones the cost model scores. It times `run()` with the shared harness (`../lib/harness.mjs`:
each build in processes of its own, three rounds in alternating order, warm-up then the
median of 10 ms batches, the islands ready first), checks both compute the same checksum,
then asks `trioxide explain --json` (auto mode) what it would pick. An unfiltered run writes
`results.json`, including each island's operation counts by kind, calls, array arguments and
bytes, which `fit.mjs` fits the weights to.

## Results

Measured on `main` at `caa9fec` (after S10's #23–#31: integer ranges for index arithmetic,
integer typed-array updates, `Math.imul` constants folded) with the refitted weights, Node
22.22.0 (V8 12.4), Intel Xeon @ 2.10 GHz × 4, islands built by the Rust 1.99.1 backend at
`opt-level = 3` without `wasm-opt`. Times are per `run()`, the median of three alternating
rounds; "faster" is the steady-state winner, and within 5% counts as a tie (either pick is
right). S9's table is in git history.

| kernel | JS | WASM | faster | model picks | benefit − cost | agrees |
|---|---|---|---|---|---|---|
| collatz | 9.30 ms | 18.70 ms | JS ×2.01 | JS | -9476221 | yes |
| fir | 1.44 ms | 1.09 ms | WASM ×1.32 | WASM | 25833 | yes |
| gamma | 1.19 ms | 2.02 ms | JS ×1.69 | JS | -214345 | yes |
| histogram | 268.6 µs | 235.4 µs | WASM ×1.14 | JS | -279522 | **no** |
| image-filters | 5.70 ms | 7.13 ms | JS ×1.25 | JS | -700053 | yes |
| int-hash | 11.48 ms | 4.26 ms | WASM ×2.70 | WASM | 1219587 | yes |
| mandelbrot | 4.75 ms | 4.63 ms | WASM ×1.03 (tie) | JS | -350905 | yes |
| matmul | 1.29 ms | 1.18 ms | WASM ×1.09 | JS | -225793 | **no** |
| noise | 8.98 ms | 6.83 ms | WASM ×1.32 | WASM | 1313855 | yes |
| number-reduction | 35.3 µs | 141.2 µs | JS ×4.00 | JS | -302854 | yes |
| per-element | 202.6 µs | 873.2 µs | JS ×4.31 | JS | -1656820 | yes |
| rounding | 4.85 ms | 2.30 ms | WASM ×2.11 | WASM | 82387 | yes |
| sdf | 476.0 µs | 668.0 µs | JS ×1.40 | JS | -325993 | yes |
| small-arrays | 125.0 µs | 2.76 ms | JS ×22.10 | JS | -2704304 | yes |
| small-helpers | 1.7 µs | 18.8 µs | JS ×10.93 | JS | -313199 | yes |
| stencil | 106.9 µs | 193.9 µs | JS ×1.81 | JS | -404910 | yes |
| tile-digest | 1.14 ms | 696.5 µs | WASM ×1.63 | WASM | 306516 | yes |
| transcendental | 3.14 ms | 3.48 ms | JS ×1.11 | JS | -184133 | yes |

| measured faster ↓ / model picks → | WASM | JS |
|---|---|---|
| WASM | 5 | 2 |
| tie (within 5%) | 0 | 1 |
| JS | 0 | 10 |

The model picks the faster side on 16 of 18 kernels. S10's backend fixes moved three of them:
`image-filters`' island went from 22.45 to 7.1 ms (JS 5.7 ms: still slower, but by 1.25×
rather than 4.4×), `histogram`'s from 3.45 ms to 235 µs (now 1.14× *faster* than JS) and
`noise`'s from 9.34 to 6.8 ms (1.32× faster). With S9's weights the model missed `histogram`,
`noise` and `rounding` (15/18 on this run, 4.74 ms per run left on the table); the refitted
ones get `noise` and `rounding` and miss `histogram` and `matmul`, two small wins (1.14× and
1.09×, 0.14 ms per run together). `fir` is new: `numeric-driver`'s FIR filter as a kernel, so
the fit sees a float kernel with typed-array arguments (without it the refit moved
`numeric-driver`'s `fir`, measured 1.11× faster in WASM, back to JS).

### What the measurements say

- **int32 work that overflows is where WASM wins**: int-hash and tile-digest (xorshift,
  `Math.imul`, sums wrapped with `| 0`), 1.7–2.2×; and `Math.round`/`floor`/`sign` (rounding,
  2.1×).
- **Float arithmetic is a tie** (mandelbrot), **float `%` is slower** (collatz, 1.9×: wasm32
  has no float remainder instruction).
- **int32-indexed accesses are no longer the problem** (matmul, 1.1× faster in WASM). S9
  listed three places where the backend still generated `f64` code, each measured by
  editing the generated crate by hand; S10 fixed all three (`../README.md`):
  - index arithmetic whose product wasn't proven int32 (`(ny * width + nx) * 4 + c`) and
    counters stepped by 4 are int32 now: image-filters' island 22.45 → 7.1 ms;
  - typed-array increments (`bins[x]++` on a `Uint32Array`) are an integer read-modify-write:
    histogram's island 3.45 ms → 235 µs;
  - `Math.imul(h, 0x9e3779b1)` folds the constant to an `i32`: noise's island 9.34 → 6.8 ms.
- The `fmod` calls S8 suspected (`trioxide_rt::array::to_uint8_clamp`'s `floor % 2.0`,
  `conv::to_uint32`'s out-of-range path) don't matter: the first only runs on exact `.5`
  values and the second only out of range. With both replaced (`(floor as u8) & 1`, and an
  `i64` cast below 2⁶³), image-filters, histogram and noise ran in 24.8, 3.58 and 9.90 ms
  against 24.4, 3.56 and 9.60 ms before: no difference beyond noise.
- **Crossings are cheap, array arguments are not**: ~8–15 ns per call through the glue,
  ~50 ns per array argument, ~0.1–0.15 ns per byte copied; a `number[]` is converted element
  by element.
- **Instantiation** of a small island takes ~0.65 ms in Node, mostly off the main thread with
  `ready()` glue (calls run the JS versions meanwhile).

### The fitted weights

`fit.mjs` solves least squares of the measured saving per run (JS − WASM) against each
island's operation counts by kind, calls, array arguments and bytes, every row scaled by the
kernel's run time. It then decides like the model: WASM when the predicted saving exceeds
the instantiation cost plus the threshold, and reports the time per run a wrong pick gives up.

**S10's refit** (fitted on a calibration run with the S9 weights, then checked on the run in
the table above) is now the default, `CostModel::default`:

| weights | kernels agreeing | time lost per run by misses | whole programs (`../README.md`) |
|---|---|---|---|
| S9's (before) | 15/18 | 4.74 ms (histogram, noise, rounding) | `mixed`: kernel; `numeric-driver`: `fir`; `image-filters`: JS |
| refitted (now) | 16/18 | 0.14 ms (histogram, matmul) | the same islands |

Both scored on the table's run; on the fitting run they agreed on 15/18 and 16/18 too.
Refitting on the table's run gives nearly the same weights and the same picks. Fitted on the
17 kernels without `fir`, the weights agreed on 16/17 but moved `numeric-driver`'s `fir` out
of WASM, where it is 1.11× faster: `fir` was added so the calibration covers it. The values
are kept as fitted, to three significant figures. `checkedIndex` keeps S9's value: no kernel
has a float-indexed access any more, so the fit has no data for it (and gives 0). When the
backend or runtime improves, rerun `node run.mjs && node fit.mjs` and update the defaults (and
this table) together.

| weight | default | S9's | meaning (ns) |
|---|---|---|---|
| `arithmetic` | −0.0914 | −0.021 | saved per float operation or comparison |
| `integer` | −0.116 | −0.0595 | saved per operation on integer-held values |
| `conversion` | −0.358 | −11.7 | saved per float operand of a bitwise operator or `Math.imul` |
| `wrapping` | 0.704 | 3.65 | saved per `(a + b) \| 0`-style wrapped operation |
| `math` | −0.0142 | 0.0242 | saved per `Math.*` call |
| `remainder` | −1.88 | −1.22 | saved per float `%` |
| `index` | −0.117 | 0.245 | saved per element access with an integer index |
| `checkedIndex` | −7.7 | −7.7 | saved per element access with a float index |
| `iteration` | 1.23 | 0.333 | saved per loop iteration |
| `crossing` | 12.9 | 13.1 | per call into an island |
| `aggregate` | 52.9 | 45.9 | per array or object argument or result, per call |
| `byte` | 0.12 | 0.169 | per byte copied |
| `numberArray` | 12 | 12 | multiplier on a `number[]`'s bytes |
| `instantiation` | 300 000 | 300 000 | per island (measured 0.65 ms, mostly off the main thread) |
| `threshold` | 0 | 0 | `benefit − cost` must exceed it in `auto` mode |
| `wasmPreferredThreshold` | −1 000 000 | −1 000 000 | the same in `wasm-preferred` mode |
| `trips` | 256 | 256 | trip count assumed for loops without a known bound |

## Default-mode reach

`node reach.mjs` lists, for every kernel here, the realistic patterns in `reach/`, the
hand-written islands of `tests/islands/` (with their `"use wasm"` directives removed) and the
examples, which top-level functions the Rust backend accepts with and without `trustTypes`.

| set | eligible by default | with `trustTypes` |
|---|---|---|
| `kernels/` (24 functions) | 24 | 24 |
| `reach/` (4) | 2 | 3 |
| `tests/islands/*/wasm` (56) | 30 | 56 |
| examples (9) | 8 | 8 |
| **total (93)** | **64** | **91** |

The analysis now proves more than PLAN §5's "For S8" note says: `number[]` elements built by
literals, `push` or `map` of proven numbers, fresh object literals' fields at struct
parameters (`reach/struct-args`), and typed arrays allocated anywhere in the program all
qualify by default. What still blocks functions by default, by missing fact:

| missing fact | blocks | owner |
|---|---|---|
| **Arguments of exported entry functions.** The host calls them; nothing proves what it passes. | `reach/entry-kernel` (`scale`) | S2/S7: the glue could check each argument's type at the call (`typeof`, `instanceof Float64Array`, plain-data checks it already does for structs) and call the JS version when one doesn't match; the island could then treat entry parameters as proven. |
| **Parameters reached through function values held in arrays.** A kernel called as `f(x)` where `f` comes from an array of functions (`[round, sign].map((f) => f(x))`) gets no argument facts. (Passed directly as a builtin's callback, `EDGES.map(popcount)`, it does.) | `tests/islands/math-edge` (10 functions) | S2: call sites whose callee is read from an array literal of functions. |
| **Typed arrays from builtins the heap doesn't model**: `subarray` views, `new T(buffer, offset, length)`, `TextEncoder.encode`, typed-array `map`. | `tests/islands/image-filter` (`brighten`, `histogram`), `bitwise` (`fnv1a`, `crc32`), `nbody` (`advance`, `energy`), `multi` (`axpy`) | S2: allocation sites for these builtins (views share the parent's element type). |
| **Elements of nested arrays and builtin-built arrays**: `number[][]` rows, `Array.from({ length }, fn)`, destructured tuples (`for (const [dx, dy, dz] of …)`). | `tests/islands/array-reduce` (4), `nbody` (`distance`, `pull`) | S2: element facts for nested array literals and `Array.from` with a callback. |
| **Fields of objects made by factories with default parameters, or copied by spread** (`{ ...body(), name }`). | `tests/islands/particles` (`step`, `collide`, `momentum`) | S2: field facts through spread and default parameters. |
| Class instances as parameters (not a fact: the ABI only passes plain objects). | `reach/class-instances` | S6 (out of v1). |
| `TypedArray.prototype.set` (not lowered). | `examples/image-filters` (`sharpen`) | S6. |

Even where a kernel qualifies, today's code generation decides more than the facts do: the
array-heavy kernels above are eligible by default and still slower in WASM (see "What the
measurements say").
