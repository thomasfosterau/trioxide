# mixed

A Vite app with all three kinds of code the partitioner has to tell apart:

- `src/view.ts`: DOM code. It stays JS: islands can't touch host APIs or strings.
- `src/kernel.ts`: a compute kernel (a digest of hashed value noise over a 256×256 tile). int32
  work per cell (xorshift rounds, `Math.imul`, sums wrapped with `| 0`) and only numbers
  crossing the boundary: it becomes a WASM island automatically, with `cellNoise` compiled in
  as an internal call.
- `src/ease.ts`: a boundary-heavy helper. A few operations per call, called 100 000 times
  from a JS loop: every call into WASM would cost more than it computes, so it stays JS.

`src/main.ts` ties them together. Its `main()` prints the digests; in a browser the module
renders them into the page.

```sh
pnpm dev             # Vite dev server (WASM is off in dev unless `dev.wasm` is set)
pnpm build           # dist/ with the kernel as a WASM island
pnpm explain         # trioxide explain (the entry is index.html's module script)
pnpm compare         # time run() JS-only, auto, and with every eligible function in WASM
```

## What `trioxide explain` says

```
src/main.ts
  run (8:8): JS
    why: ineligible: returns values of different types (or a value on some paths only)
    failed rules:
      8:17  returns values of different types (or a value on some paths only)
      9:27  creates an array literal
      10:22  reads `SEEDS`, which is defined outside the function (only module-level constants can be read)
  main (19:8): JS
    why: ineligible: calls `run`, which can't be compiled to WASM
    failed rules:
      20:22  calls `run`, which can't be compiled to WASM
      8:17  in `run`: returns values of different types (or a value on some paths only)
      9:27  in `run`: creates an array literal
      10:22  in `run`: reads `SEEDS`, which is defined outside the function (only module-level constants can be read)

src/ease.ts
  ease (5:8): JS
    why: the cost model keeps it in JS: benefit − cost = -1120000 ≤ threshold 0
    cost model: benefit -20000 − cost 1100000 = -1120000 ≤ threshold 0
      benefit: 600000 operations over 100000 call(s)
      cost: crossings 800000 + marshalling 0 (0 bytes) + instantiation 300000
    passes:
      fold skipped at 6:13, 6:22: the call's result depends on what its callers pass (`passes.specialize` is off)

src/kernel.ts
  tileDigest (10:8): WASM in island `kernel`
    why: chosen by the cost model; exported by the island
    cost model: benefit 2752706 − cost 300024 = 2452682 > threshold 0
      benefit: 12585222 operations over 3 call(s)
      cost: crossings 24 + marshalling 0 (0 bytes) + instantiation 300000
    passes:
      applied: inline ×2
  cellNoise (22:8): WASM in island `kernel`
    why: compiled into the island as a callee (its JS version is kept too)
    cost model: benefit 2752706 − cost 300024 = 2452682 > threshold 0
      benefit: 12585222 operations over 3 call(s)
      cost: crossings 24 + marshalling 0 (0 bytes) + instantiation 300000

src/view.ts
  render (4:8): JS
    why: ineligible: can't prove what parameter `root` receives
    failed rules:
      4:24  can't prove what parameter `root` receives
      4:43  parameter `lines` is an array of anything, not of numbers
      5:16  uses the global `document` (a host API)
```

- `tileDigest` becomes WASM: its int32 operations, and above all the sums wrapped with `| 0`
  (which leave V8's int32 fast path when they overflow, while WASM wraps natively), save more
  than the island's crossings and instantiation cost. `cellNoise` joins the island as a
  callee; its JS version stays for the `ready()` fallback.
- `ease` stays JS: the model expects no saving on its floating-point arithmetic, while its
  100 000 calls would each pay a crossing.
- `run`, `main` and `render` are ineligible, and `explain` lists every rule each one fails
  with its position (strings, arrays of strings, module state, `document`).
- The `passes` lines are the JS passes' decisions inside each function.

## Same behaviour either way

The differential suite runs this example's `main()` types-stripped, as trioxide's JS, and with
the islands `auto` picks (`tests/differential`, legs a, b and d); all print the same. `pnpm
compare` times `run()` three ways (Node 22, the median of three alternating rounds):

| build | islands | `run()` |
|---|---|---|
| JS only | — | 1.30 ms |
| auto | kernel | 0.82 ms |
| every cluster | ease, kernel | 1.41 ms |

Moving the kernel makes `run()` about 1.6× as fast; moving `ease` too gives part of that
back, as the model predicts.
