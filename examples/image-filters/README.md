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

The `src/filters.ts` part of `pnpm explain`:

```
src/filters.ts:49:3  in `sharpen`: calls the array method `.set()`
    passes:
      applied: inline ×10, fold ×2
  main (24:8): JS
    why: ineligible: creates an array literal
    failed rules:
      25:17  creates an array literal
      26:3  calls `run`, which can't be compiled to WASM
      8:17  in `run`: returns values of different types (or a value on some paths only)
      10:16  in `run`: creates an array literal
      15:3  in `run`: calls `sharpen`, which can't be compiled to WASM
      src/filters.ts:49:3  in `sharpen`: calls the array method `.set()`
    passes:
      closure-hoist skipped at 26:17: the closure captures `names`

src/filters.ts
  grayscale (5:8): JS
    why: the cost model keeps it in JS: benefit − cost = -3633193 ≤ threshold 0
    cost model: benefit -3271680 − cost 361513 = -3633193 ≤ threshold 0
      benefit: 1459200 operations over 1 call(s)
      cost: crossings 8 + marshalling 61505 (614400 bytes) + instantiation 300000
    passes:
      fold skipped at 6:27: the expression's single value depends on what its callers pass (`passes.specialize` is off)
  invert (15:8): JS
    why: the cost model keeps it in JS: benefit − cost = -3625513 ≤ threshold 0
    cost model: benefit -3264000 − cost 361513 = -3625513 ≤ threshold 0
      benefit: 1305600 operations over 1 call(s)
      cost: crossings 8 + marshalling 61505 (614400 bytes) + instantiation 300000
    passes:
      fold skipped at 16:27: the expression's single value depends on what its callers pass (`passes.specialize` is off)
  boxBlur (24:8): WASM in island `filters`
    why: chosen by the cost model; exported by the island
    cost model: benefit 1040700 − cost 392298 = 648402 > threshold 0
      benefit: 39245520 operations over 1 call(s)
      cost: crossings 8 + marshalling 92290 (921600 bytes) + instantiation 300000
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
cost model scores them. `sharpen` is ineligible: it calls `.set()`, which the backend doesn't
lower.

**`boxBlur` is a misprediction today.** The analysis now proves its loop counters and its
index arithmetic int32 (with `width` and `height` constant, `(ny * width + nx) * 4 + c` is
an int32 everywhere), so the model counts 25 million int32 operations at 0.3 ns saved each
against 2.5 million int32-indexed accesses at 2.5 ns lost each, and expects WASM to win by
about 0.65 ms. Measured, the island is slower: `pnpm compare` (Node 22, the median of three
alternating rounds):

| build | islands | `run()` |
|---|---|---|
| JS only | — | 32.08 ms |
| auto | filters (`boxBlur`) | 40.55 ms |
| every cluster | filters, image | 50.44 ms |

`grayscale` and `invert` still stay JS: their counters (`i += 4` against `pixels.length`)
are `f64` in the island, so every access is a float-indexed, checked one (7–8 ns lost each).
Refitting the weights doesn't fix `boxBlur` (the fit picks it too); what the model can't see
is in the generated code. `benchmarks/README.md` ("Cost-model refit") has the measurements
and what each backend fix would change. Until then, `"use js"` on `boxBlur`, or
`"mode": "js-only"`, keeps the program at its JS speed.

## Same behaviour either way

The differential suite runs this example's `main()` types-stripped, as trioxide's JS, and with
the islands `auto` picks (`tests/differential`, legs a, b and d); all print the same.
`pnpm compare` checks the every-cluster build prints the same too.
