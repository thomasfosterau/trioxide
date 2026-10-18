# Partition benchmarks

Calibration data for the partitioner's cost model (`docs/PLAN.md` §5). Each kernel in
`kernels/<name>/` is a `kernel.ts` and an `input.ts` whose exported `run()` does the work and
returns a checksum (and whose `main()` prints it). The kernels cover a range of
compute/boundary ratios: scalar kernels called once, the same kind of arithmetic called per
element from a JS loop, typed-array image filters, a `number[]` reduction, small helpers.

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

| kernel | JS | WASM | faster | model picks | benefit − cost (ns) | agrees |
|---|---|---|---|---|---|---|
| collatz | 9.92 ms | 9.92 ms | JS ×1.00 (tie) | JS | -78445 | yes |
| gamma | 1.26 ms | 2.18 ms | JS ×1.73 | JS | -1780095 | yes |
| histogram | 282.7 µs | 3.87 ms | JS ×13.68 | JS | -3043686 | yes |
| image-filters | 5.94 ms | 17.49 ms | JS ×2.94 | JS | -16775869 | yes |
| int-hash | 11.44 ms | 11.74 ms | JS ×1.03 (tie) | WASM | 419976 | yes |
| mandelbrot | 5.04 ms | 4.97 ms | WASM ×1.01 (tie) | WASM | 543200 | yes |
| matmul | 1.61 ms | 14.61 ms | JS ×9.05 | JS | -12966650 | yes |
| number-reduction | 37.9 µs | 602.4 µs | JS ×15.89 | JS | -706083 | yes |
| per-element | 212.4 µs | 908.9 µs | JS ×4.28 | JS | -1900000 | yes |
| rounding | 5.34 ms | 4.94 ms | WASM ×1.08 | JS | -300024 | no |
| sdf | 539.8 µs | 500.4 µs | WASM ×1.08 | JS | -651976 | no |
| small-arrays | 129.6 µs | 2.77 ms | JS ×21.39 | JS | -3317600 | yes |
| small-helpers | 2.0 µs | 2.0 µs | WASM ×1.01 (tie) | JS | -699704 | yes |
| stencil | 107.8 µs | 197.4 µs | JS ×1.83 | JS | -1292542 | yes |
| transcendental | 3.36 ms | 3.40 ms | JS ×1.01 (tie) | JS | -652024 | yes |

| measured faster ↓ / model picks → | WASM | JS |
|---|---|---|
| WASM | 0 | 2 |
| tie (within 5%) | 2 | 3 |
| JS | 0 | 8 |

The model picks the faster side (or one of two equal sides) on 13 of 15 kernels. The two
misses gain 8% in WASM, 0.4 ms and 0.04 ms per run: less than instantiating an island
(~0.65 ms measured), which the model charges once per program, so for a program that runs
them once JS is still the better choice. Scalar timings move by up to ±10% between runs; the
array-heavy results are stable.

### What the measurements say

- **Scalar code is a tie.** V8's optimising compiler and the islands (LLVM `-O3`) run
  floating-point and integer arithmetic, `Math.*` and branches at the same speed, within a
  few percent either way (collatz, int-hash, mandelbrot, sdf, rounding, transcendental).
- **Element accesses are slower in WASM**, by about 2 ns each when the island indexes with an
  int32 (gamma, stencil) and about 7 ns when it indexes with a float that it converts and
  checks per access (matmul, image filters, histogram). Causes, for S6/S7:
  - `trioxide_rt::array::to_uint8_clamp` computes `floor % 2.0` in `f64`, which wasm32 has no
    instruction for: every `Uint8ClampedArray` store calls `fmod`. `(floor as u8) & 1` is
    exact there (the value is in 0..=254).
  - Typed-array increments (`bins[x]++` on a `Uint32Array`) go through `f64` (load, convert,
    add, `ToUint32`, store), so the loop-carried dependency through memory is several times
    longer than V8's integer path: histogram is 13× slower.
  - Loop counters bounded by anything but a literal (`i < n`, `i < a.length`) are `f64` in
    the island, so their accesses are float-indexed and checked. Their binding's range fact is
    `[0, ∞)`: the whole-program symbol table keeps the values written in early rounds,
    before the bound's range is known, and loop heads widen straight to infinity. (A
    widening ladder at the int32 and uint32 limits fixes the head ranges, but not the symbol
    table; that needs S2's narrowing to reach symbol facts.)
- **Crossings are cheap, array arguments are not.** A call costs ~7–12 ns more through the
  glue; each array argument adds ~60 ns (arena allocation, the aliasing and plain-view
  checks, the views it creates), and copying adds ~0.1 ns per byte. A `number[]` is converted
  element by element, ~10 ns per element.
- **Instantiation** of a small island takes ~0.65 ms in Node.

### The fitted weights

`fit.mjs` solves least squares of the measured saving per run (JS − WASM) against each
island's operation counts, calls, array arguments and bytes, every row scaled by the
kernel's run time. The terms are collinear on fifteen kernels (it fits a negative cost per
byte), so the defaults (`CostModel::default`, `costModel` in `trioxide.json`) are the fitted
values rounded to physically sensible ones:

| weight | fitted | default | meaning (ns) |
|---|---|---|---|
| `arithmetic` | 0.000 | 0 | saved per floating-point operation or comparison |
| `integer` | 0.030 | 0.04 | saved per integer or bitwise operation, `++`, `--` |
| `math` | 0.013 | 0 | saved per `Math.*` call |
| `index` | −2.4 | −2 | saved per element access with an int32 index |
| `checkedIndex` | −6.9 | −7 | saved per element access with a float index |
| `iteration` | −0.05 | 0 | saved per loop iteration |
| `crossing` | 7.0 | 12 | per call into an island |
| `aggregate` | 64 | 50 | per array or object argument or result, per call |
| `byte` | (−0.67) | 0.1 | per byte copied |
| `numberArray` | — | 12 | multiplier on a `number[]`'s bytes |
| `instantiation` | — | 700 000 | per island (measured) |
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
| `kernels/` (19 functions) | 19 | 19 |
| `reach/` (4) | 2 | 3 |
| `tests/islands/*/wasm` (56) | 30 | 56 |
| examples (9) | 8 | 8 |
| **total (88)** | **59** | **86** |

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
