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

Measured on `main` after #15–#19 (with the int32-index fix to the metrics from S9), Node
22.22.0 (V8 12.4), Intel Xeon @ 2.10 GHz × 4, islands built by the Rust 1.99.1 backend at
`opt-level = 3` without `wasm-opt`. Times are per `run()`; "faster" is the steady-state
winner, and within 5% counts as a tie (either pick is right). S8's numbers, from before the
analysis proved most loop counters int32, are in git history.

| kernel | JS | WASM | faster | model picks | benefit − cost | agrees |
|---|---|---|---|---|---|---|
| collatz | 8.83 ms | 16.68 ms | JS ×1.89 | JS | -14117547 | yes |
| gamma | 1.11 ms | 1.98 ms | JS ×1.79 | JS | -1623900 | yes |
| histogram | 254.2 µs | 3.59 ms | JS ×14.11 | JS | -4258519 | yes |
| image-filters | 5.41 ms | 22.54 ms | JS ×4.16 | JS | -9408558 | yes |
| int-hash | 10.30 ms | 4.61 ms | WASM ×2.24 | WASM | 8999992 | yes |
| mandelbrot | 4.44 ms | 4.39 ms | WASM ×1.01 (tie) | JS | -2587470 | yes |
| matmul | 1.41 ms | 983.0 µs | WASM ×1.43 | JS | -3574163 | **no** |
| noise | 8.27 ms | 9.46 ms | JS ×1.14 | WASM | 10605824 | **no** |
| number-reduction | 34.2 µs | 502.7 µs | JS ×14.69 | JS | -306166 | yes |
| per-element | 306.5 µs | 1.70 ms | JS ×5.54 | JS | -1135000 | yes |
| rounding | 4.59 ms | 2.19 ms | WASM ×2.09 | WASM | 1399992 | yes |
| sdf | 460.8 µs | 622.9 µs | JS ×1.35 | JS | -257933 | yes |
| small-arrays | 118.4 µs | 2.47 ms | JS ×20.84 | JS | -3512000 | yes |
| small-helpers | 1.7 µs | 16.7 µs | JS ×10.11 | JS | -307800 | yes |
| stencil | 107.6 µs | 164.1 µs | JS ×1.53 | JS | -994801 | yes |
| tile-digest | 1.10 ms | 645.0 µs | WASM ×1.70 | WASM | 2452682 | yes |
| transcendental | 2.98 ms | 3.26 ms | JS ×1.10 | JS | -150008 | yes |

| measured faster ↓ / model picks → | WASM | JS |
|---|---|---|
| WASM | 3 | 1 |
| tie (within 5%) | 0 | 1 |
| JS | 1 | 11 |

The model picks the faster side on 15 of 17 kernels. Since S8 measured, the analysis proves
loop counters bounded by parameters and `.length` int32, which moved three kernels:
`rounding` (S8's miss) now counts its counter work as int32 and is picked; `matmul` became
1.4× faster in WASM (int32-indexed accesses are no longer slower) while the model, with
`index` at −2.5 ns, still keeps it JS; and `noise` became a model pick (its counters are
int32) while the island is still 1.1× slower (see below).

### What the measurements say

- **int32 work that overflows is where WASM wins**: int-hash and tile-digest (xorshift,
  `Math.imul`, sums wrapped with `| 0`), 1.7–2.2×; and `Math.round`/`floor`/`sign` (rounding,
  2.1×).
- **Float arithmetic is a tie** (mandelbrot), **float `%` is slower** (collatz, 1.9×: wasm32
  has no float remainder instruction).
- **int32-indexed accesses are no longer the problem** (matmul, 1.4× faster in WASM); what is
  slow is code the backend still generates in `f64`. Each of these was measured by editing the
  generated crate by hand (not committed); `../README.md` has the details for S6/S7:
  - index arithmetic whose product isn't proven int32 (`(ny * width + nx) * 4 + c` with
    `width` a parameter) runs in `f64` and every access is checked, and so does a counter
    stepped by 4 (`grayscale`): image-filters' island goes
    from 23.3 ms to 6.5 ms (JS: 8.0 ms in the same run) with int32 indices and counter;
  - typed-array increments go through `f64` (`bins[x]++` on a `Uint32Array`): histogram's
    island goes from 3.46 ms to 0.21 ms (JS: 0.26 ms) with an integer read-modify-write;
  - `Math.imul(h, 0x9e3779b1)` converts both operands with `ToInt32` at run time because the
    constant is above 2³¹: noise's island goes from 9.9 ms to 6.25 ms (JS: 8.3 ms) with the
    constant folded to an `i32`.
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
the instantiation cost plus the threshold.

**The refit was not applied.** On these measurements the defaults and the fit both pick right
on 15 of 17 kernels, and the fit's misses cost more (`node fit.mjs`, `node fit.mjs --weights`
with the defaults):

| weights | agree | misses (time lost per run) |
|---|---|---|
| defaults (`CostModel::default`) | 15/17 | matmul (0.42 ms), noise (1.19 ms): 1.61 ms |
| fitted (ridge 10⁻³) | 15/17 | noise (1.19 ms), rounding (2.40 ms): 3.59 ms |

The fitted weights were `arithmetic` 0.07, `integer` 0.17, `conversion` −5.3, `wrapping` 1.6,
`math` −0.11, `remainder` −0.41, `index` 0.22, `checkedIndex` −8.2, `iteration` −1.05,
`crossing` 14.8, `aggregate` 48.6, `byte` 0.14 (other ridge values give 14–15/17 and lose more).
The misses that remain are the backend's `f64` paths above, which no weight on these
operation kinds describes; once S6 fixes them, rerun `node run.mjs && node fit.mjs` and update
the defaults (and this table) together. The current defaults:

| weight | default | meaning (ns) |
|---|---|---|
| `arithmetic` | −0.05 | saved per float operation or comparison |
| `integer` | 0.3 | saved per operation on int32-held values |
| `conversion` | −6 | saved per float operand of a bitwise operator or `Math.imul` |
| `wrapping` | 2 | saved per `(a + b) \| 0`-style wrapped operation |
| `math` | 0 | saved per `Math.*` call |
| `remainder` | −1.5 | saved per float `%` |
| `index` | −2.5 | saved per element access with an int32 index |
| `checkedIndex` | −7 | saved per element access with a float index |
| `iteration` | 0 | saved per loop iteration |
| `crossing` | 8 | per call into an island |
| `aggregate` | 65 | per array or object argument or result, per call |
| `byte` | 0.1 | per byte copied |
| `numberArray` | 12 | multiplier on a `number[]`'s bytes |
| `instantiation` | 300 000 | per island (measured 0.65 ms, mostly off the main thread) |
| `threshold` | 0 | `benefit − cost` must exceed it in `auto` mode |
| `wasmPreferredThreshold` | −1 000 000 | the same in `wasm-preferred` mode |
| `trips` | 256 | trip count assumed for loops without a known bound |

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
array-heavy kernels above are eligible by default and still slower in WASM (see "What the
measurements say").
