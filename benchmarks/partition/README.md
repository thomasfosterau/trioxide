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
cargo build -p trioxide_cli --release   # optional; target/debug works too
node run.mjs              # time JS vs WASM, ask the cost model, print the confusion table
node fit.mjs              # least-squares fit of the weights to results.json
node fit.mjs --weights w.json   # evaluate other weights on the same measurements
node reach.mjs            # what qualifies with and without trustTypes, and what blocks it
node compare.mjs src/main.ts    # from a project: JS-only vs auto vs every cluster as WASM
```

`run.mjs` builds every kernel twice: JS-only (`mode: "js-only"`), and with every eligible
cluster as an island (`wasm-preferred` with no threshold), so the islands are exactly the
ones the cost model scores. It times `run()` in fresh Node processes after the islands are
ready (warm-up, then the median of 25 ms batches; the median of three processes), checks
both print the same checksum, then asks `trioxide explain --json` (auto mode) what it would
pick. It writes `results.json`, including each island's operation counts by kind, calls,
array arguments and bytes, which `fit.mjs` fits the weights to.

## Results

Node 22.22.0 (V8 12.4), Intel Xeon @ 2.10 GHz, islands built by the Rust 1.99.1 backend at
`opt-level = 3` without `wasm-opt`. Times are per `run()`; "faster" is the steady-state
winner, and within 5% counts as a tie (either pick is right).

| kernel | JS | WASM | faster | model picks | benefit − cost | agrees |
|---|---|---|---|---|---|---|
| collatz | 10.09 ms | 19.23 ms | JS ×1.91 | JS | -14128047 | yes |
| gamma | 1.27 ms | 2.15 ms | JS ×1.69 | JS | -1623900 | yes |
| histogram | 276.4 µs | 3.81 ms | JS ×13.78 | JS | -4258519 | yes |
| image-filters | 5.71 ms | 17.56 ms | JS ×3.07 | JS | -18232962 | yes |
| int-hash | 11.42 ms | 5.22 ms | WASM ×2.19 | WASM | 8299992 | yes |
| mandelbrot | 4.93 ms | 5.10 ms | JS ×1.03 (tie) | JS | -5307023 | yes |
| matmul | 1.40 ms | 14.55 ms | JS ×10.38 | JS | -13136257 | yes |
| noise | 9.28 ms | 15.25 ms | JS ×1.64 | JS | -11727952 | yes |
| number-reduction | 40.8 µs | 610.3 µs | JS ×14.95 | JS | -306166 | yes |
| per-element | 218.2 µs | 831.1 µs | JS ×3.81 | JS | -1135000 | yes |
| rounding | 5.27 ms | 3.53 ms | WASM ×1.49 | JS | -700008 | **no** |
| sdf | 573.9 µs | 705.4 µs | JS ×1.23 | JS | -384038 | yes |
| small-arrays | 133.6 µs | 2.49 ms | JS ×18.61 | JS | -3512000 | yes |
| small-helpers | 2.1 µs | 17.3 µs | JS ×8.43 | JS | -307800 | yes |
| stencil | 110.6 µs | 204.4 µs | JS ×1.85 | JS | -994801 | yes |
| tile-digest | 1.17 ms | 725.0 µs | WASM ×1.62 | WASM | 2452682 | yes |
| transcendental | 3.54 ms | 3.77 ms | JS ×1.07 | JS | -360008 | yes |

| measured faster ↓ / model picks → | WASM | JS |
|---|---|---|
| WASM | 2 | 1 |
| tie (within 5%) | 0 | 1 |
| JS | 0 | 13 |

The model picks the faster side on 16 of 17 kernels. The miss, rounding, gains 1.5× in WASM
from `Math.round`/`Math.floor`/`Math.sign` specifically, which one `math` weight can't tell
apart from `Math.atan` and `Math.log2` (transcendental, slower in WASM). Scalar timings move by
a few percent between runs; the array-heavy results are stable.

### What the measurements say

- **int32 work that overflows is where WASM wins.** int-hash and tile-digest (xorshift,
  `Math.imul`, sums wrapped with `| 0`) run 1.6–2.2× faster: V8 leaves its int32 fast path when
  a sum overflows, WASM wraps natively. noise does the same hashing on float coordinates
  (loop counters bounded by parameters, see below) and is 1.6× *slower*: every
  `Math.imul(x, …)` does a float `ToInt32`.
- **Float arithmetic is a tie** (mandelbrot), **float `%` is slower** (collatz, 1.9×: wasm32
  has no float remainder instruction, so `%` calls `fmod`), and so are most `Math` calls but
  `round`/`floor`/`sign`.
- **Element accesses are slower in WASM**, by about 2.5 ns each when the island indexes with
  an int32 (gamma, stencil) and about 7 ns with a float index that it converts and checks per
  access (matmul, image filters, histogram). Causes, for S6/S7:
  - `trioxide_rt::array::to_uint8_clamp` computes `floor % 2.0` in `f64`, which wasm32 has no
    instruction for: every `Uint8ClampedArray` store calls `fmod`. `(floor as u8) & 1` is
    exact there (the value is in 0..=254). `conv::to_uint32` (and `to_int32` beyond ±2³¹)
    take the same `%` path.
  - Typed-array increments (`bins[x]++` on a `Uint32Array`) go through `f64` (load, convert,
    add, `ToUint32`, store), so the loop-carried dependency through memory is several times
    longer than V8's integer path: histogram is 14× slower.
  - Loop counters bounded by anything but a literal or a constant (`i < n`, `i < a.length`)
    are `f64` in the island, so their accesses are float-indexed and checked, and integer work
    on them converts. Their binding's range fact is `[0, ∞)`: the whole-program symbol table
    keeps the values written in early rounds, before the bound's range is known, and loop
    heads widen straight to infinity. (A widening ladder at the int32 and uint32 limits fixes
    the head ranges, but not the symbol table; that needs S2's narrowing to reach symbol
    facts.)
- **Crossings are cheap, array arguments are not.** A call costs ~7 ns more through the glue;
  each array argument adds ~60 ns (arena allocation, the aliasing and plain-view checks, the
  views it creates), and copying adds ~0.1 ns per byte. A `number[]` is converted element by
  element, ~10 ns per element.
- **Instantiation** of a small island takes ~0.65 ms in Node, mostly off the main thread with
  `ready()` glue (calls run the JS versions meanwhile).

### The fitted weights

`fit.mjs` solves least squares of the measured saving per run (JS − WASM) against each
island's operation counts by kind, calls, array arguments and bytes, every row scaled by the
kernel's run time. Twelve weights on seventeen kernels are collinear (the fit puts `wrapping`'s
gain into `integer` and `iteration`, and gives bytes a negative cost), so the defaults
(`CostModel::default`, `costModel` in `trioxide.json`) are the fitted values rounded to
physically sensible ones; with them the model's picks match the fit's on every kernel and
the predicted savings are within a few times of the measured ones.

| weight | fitted | default | meaning (ns) |
|---|---|---|---|
| `arithmetic` | −0.07 | −0.05 | saved per float operation or comparison |
| `integer` | 0.30 | 0.3 | saved per operation on int32-held values |
| `conversion` | −6.1 | −6 | saved per float operand of a bitwise operator or `Math.imul` |
| `wrapping` | (−1.9) | 2 | saved per `(a + b) \| 0`-style wrapped operation |
| `math` | 0.02 | 0 | saved per `Math.*` call |
| `remainder` | −1.7 | −1.5 | saved per float `%` |
| `index` | −2.3 | −2.5 | saved per element access with an int32 index |
| `checkedIndex` | −6.6 | −7 | saved per element access with a float index |
| `iteration` | (0.93) | 0 | saved per loop iteration |
| `crossing` | 7.5 | 8 | per call into an island |
| `aggregate` | 56 | 65 | per array or object argument or result, per call |
| `byte` | (−0.53) | 0.1 | per byte copied |
| `numberArray` | — | 12 | multiplier on a `number[]`'s bytes |
| `instantiation` | — | 300 000 | per island (measured 0.65 ms, mostly off the main thread) |
| `threshold` | — | 0 | `benefit − cost` must exceed it in `auto` mode |
| `wasmPreferredThreshold` | — | −1 000 000 | the same in `wasm-preferred` mode |
| `trips` | — | 256 | trip count assumed for loops without a known bound |

When the backend or runtime improves, rerun `node run.mjs && node fit.mjs` and update the
defaults (and this table) together.

## Default-mode reach

`node reach.mjs` lists, for every kernel here, the realistic patterns in `reach/`, the
hand-written islands of `tests/islands/` (with their `"use wasm"` directives removed) and the
examples, which top-level functions the Rust backend accepts with and without `trustTypes`.

| set | eligible by default | with `trustTypes` |
|---|---|---|
| `kernels/` (23 functions) | 23 | 23 |
| `reach/` (4) | 2 | 3 |
| `tests/islands/*/wasm` (56) | 30 | 56 |
| examples (9) | 8 | 8 |
| **total (92)** | **63** | **90** |

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
array-heavy kernels above are eligible by default and still slower in WASM, and so is int32
work on parameter-bounded counters (see "What the measurements say").
