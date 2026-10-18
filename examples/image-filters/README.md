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
src/filters.ts
  grayscale (5:8): JS
    why: the cost model keeps it in JS: benefit − cost = -3945832 ≤ threshold 0
    cost model: benefit -3541939 − cost 403893 = -3945832 ≤ threshold 0
      benefit: 1459200 operations over 1 call(s)
      cost: crossings 13 + marshalling 103880 (614400 bytes) + instantiation 300000
    passes:
      fold skipped at 6:27: the expression's single value depends on what its callers pass (`passes.specialize` is off)
  invert (15:8): JS
    why: the cost model keeps it in JS: benefit − cost = -3942606 ≤ threshold 0
    cost model: benefit -3538714 − cost 403893 = -3942606 ≤ threshold 0
      benefit: 1305600 operations over 1 call(s)
      cost: crossings 13 + marshalling 103880 (614400 bytes) + instantiation 300000
    passes:
      fold skipped at 16:27: the expression's single value depends on what its callers pass (`passes.specialize` is off)
  boxBlur (24:8): JS
    why: the cost model keeps it in JS: benefit − cost = -632751 ≤ threshold 0
    cost model: benefit -176896 − cost 455855 = -632751 ≤ threshold 0
      benefit: 39245520 operations over 1 call(s)
      cost: crossings 13 + marshalling 155842 (921600 bytes) + instantiation 300000
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

The model keeps all three in JS, and that is the faster choice. `boxBlur` is the close one:
the analysis proves its loop counters and its index arithmetic int32 (with `width` and
`height` constant, `(ny * width + nx) * 4 + c` is an int32 everywhere), and before the S9
refit the model counted that int32 work as a win and picked it. Measured, the island is
slower, so the refitted weights (`benchmarks/README.md`, "Cost-model refit") keep it in JS.
`grayscale` and `invert` stay JS by a wide margin: their counters (`i += 4` against
`pixels.length`) are `f64` in the island, so every access is a float-indexed, checked one.

`run()`, Node 22, each build in processes of its own over alternating rounds (`pnpm compare`
with three rounds; eleven for the `boxBlur` row, whose processes vary more):

| build | islands | `run()` |
|---|---|---|
| JS only, and `auto` (the same output) | — | 32.9 ms |
| `boxBlur` in WASM (what the weights before the refit picked) | filters | 40.9 ms (0.79×; 0.64–1.26× over 11 rounds) |
| every cluster | filters, image | 48.3 ms |

What makes these islands slow is code the backend still generates in `f64` (see
`benchmarks/README.md`); once S6 fixes it, rerun `benchmarks/partition` and refit, and the
same `explain` output shows where the balance tips.

## Same behaviour either way

The differential suite runs this example's `main()` types-stripped, as trioxide's JS, and with
the islands `auto` picks (`tests/differential`, legs a, b and d); all print the same.
`pnpm compare` checks the every-cluster build prints the same too.
