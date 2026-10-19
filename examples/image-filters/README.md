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
    why: the cost model keeps it in JS: benefit − cost = -428813 ≤ threshold 0
    cost model: benefit -55020 − cost 373794 = -428813 ≤ threshold 0
      benefit: 1459200 operations over 1 call(s)
      cost: crossings 13 + marshalling 73781 (614400 bytes) + instantiation 300000
    passes:
      fold skipped at 6:27: the expression's single value depends on what its callers pass (`passes.specialize` is off)
  invert (15:8): JS
    why: the cost model keeps it in JS: benefit − cost = -414774 ≤ threshold 0
    cost model: benefit -40980 − cost 373794 = -414774 ≤ threshold 0
      benefit: 1305600 operations over 1 call(s)
      cost: crossings 13 + marshalling 73781 (614400 bytes) + instantiation 300000
    passes:
      fold skipped at 16:27: the expression's single value depends on what its callers pass (`passes.specialize` is off)
  boxBlur (24:8): JS
    why: the cost model keeps it in JS: benefit − cost = -700053 ≤ threshold 0
    cost model: benefit -289343 − cost 410711 = -700053 ≤ threshold 0
      benefit: 39245520 operations over 1 call(s)
      cost: crossings 13 + marshalling 110698 (921600 bytes) + instantiation 300000
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

The model keeps all three in JS, and that is the faster choice. The analysis proves their
loop counters and index arithmetic int32 (with `width` and `height` constant,
`(ny * width + nx) * 4 + c` is an int32 everywhere, and since S10 so is a counter stepped by 4
against `pixels.length`), and S10's backend computes them in `i32`: `boxBlur`'s island went
from 22.5 to 7.1 ms in `benchmarks/partition`'s `image-filters` kernel. It is still slower
than JS (5.7 ms there), and the refitted weights (`benchmarks/README.md`, "Cost-model
refit") keep all three in JS.

`run()`, Node 22, each build in processes of its own over three alternating rounds (`pnpm
compare`):

| build | islands | `run()` |
|---|---|---|
| JS only | — | 32.9 ms |
| `auto` (the same output: no islands) | — | 35.2 ms |
| every cluster | filters, image | 50.0 ms |

`auto` and JS-only emit the same code, so their difference is the processes' noise. The
every-cluster build is slower, as the kernel measurement says the islands are.

## Same behaviour either way

The differential suite runs this example's `main()` types-stripped, as trioxide's JS, and with
the islands `auto` picks (`tests/differential`, legs a, b and d); all print the same.
`pnpm compare` checks the every-cluster build prints the same too.
