# image-filters

A small CLI app: a procedural RGBA test image (`src/image.ts`) run through four filters
(`src/filters.ts`: grayscale, invert, a 3×3 box blur and sharpening), printing a checksum
after each step (`src/main.ts`).

```sh
pnpm start           # trioxide build src/main.ts --out-dir dist, then run main()
pnpm explain         # trioxide explain --entry src/main.ts
pnpm compare         # time run() JS-only, auto, and with every eligible function in WASM
```

## What `trioxide explain` says

```
src/filters.ts
  grayscale (5:8): JS
    why: the cost model keeps it in JS: benefit − cost = -3968670 ≤ threshold 0
    cost model: benefit -3207168 − cost 761502 = -3968670 ≤ threshold 0
      benefit: 1459200 operations over 1 call(s)
      cost: crossings 12 + marshalling 61490 (614400 bytes) + instantiation 700000
    passes:
      fold skipped at 6:27: the expression's single value depends on what its callers pass (`passes.specialize` is off)
  invert (15:8): JS
    why: the cost model keeps it in JS: benefit − cost = -3968670 ≤ threshold 0
    cost model: benefit -3207168 − cost 761502 = -3968670 ≤ threshold 0
      benefit: 1305600 operations over 1 call(s)
      cost: crossings 12 + marshalling 61490 (614400 bytes) + instantiation 700000
    passes:
      fold skipped at 16:27: the expression's single value depends on what its callers pass (`passes.specialize` is off)
  boxBlur (24:8): JS
    why: the cost model keeps it in JS: benefit − cost = -16775869 ≤ threshold 0
    cost model: benefit -15983597 − cost 792272 = -16775869 ≤ threshold 0
      benefit: 39245520 operations over 1 call(s)
      cost: crossings 12 + marshalling 92260 (921600 bytes) + instantiation 700000
    passes:
      inline skipped at 25:23, 26:25, 34:33, 34:58, 35:32, 40:18, 42:16, 42:47: the binding's single value depends on what its callers pass (`passes.specialize` is off)
      fold skipped at 25:23, 26:25, 34:33, 34:58, 35:32, 40:18, 42:16, 42:47: the expression's single value depends on what its callers pass (`passes.specialize` is off)
  sharpen (48:8): JS
    why: ineligible: calls the array method `.set()`
    failed rules:
      49:3  calls the array method `.set()`
    passes:
      inline skipped at 50:23, 51:25, 52:23, 55:65, 55:86: the binding's single value depends on what its callers pass (`passes.specialize` is off)
      fold skipped at 50:23, 51:25, 52:23, 55:65, 55:86: the expression's single value depends on what its callers pass (`passes.specialize` is off)
```

`grayscale`, `invert` and `boxBlur` are eligible in the default mode (every typed array that
reaches them is allocated in the program, and their other arguments are constants), so the
cost model scores them, and keeps them in JS. Their benefit is negative: element accesses
are slower in today's islands than in V8's optimised JS, by ~2 ns with an int32 index and
~7 ns with a float index (which these loops get, as their counters are bounded by parameters
and array lengths), and every `Uint8ClampedArray` store pays for a software `fmod` in the
runtime. `benchmarks/partition/README.md` has the measurements and the causes. `sharpen` is
ineligible: it calls `.set()`, which the backend doesn't lower.

That is the cost model working as intended. `pnpm compare` (Node 22):

| build | islands | `run()` |
|---|---|---|
| JS only | — | 35.2 ms |
| auto | — | 34.5 ms |
| every cluster | filters, image | 62.5 ms |

Forcing them into WASM (`"use wasm"`, an `overrides` entry with `"target": "wasm"`, or
`wasm-preferred` with a very low `costModel.wasmPreferredThreshold`) makes the program almost
twice as slow. When the backend gets int32 counters for parameter-bounded loops and a
cheaper clamped store, rerun `benchmarks/partition` and refit the weights: the same
explain output then shows where the balance tips.

## Same behaviour either way

The differential suite runs this example's `main()` types-stripped, as trioxide's JS, and with
the islands `auto` picks (`tests/differential`, legs a, b and d); all print the same.
`pnpm compare` checks the every-cluster build prints the same too.
