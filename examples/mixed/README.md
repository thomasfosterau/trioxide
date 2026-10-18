# mixed

A Vite app with all three kinds of code the partitioner has to tell apart:

- `src/view.ts`: DOM code. It stays JS: islands can't touch host APIs or strings.
- `src/kernel.ts`: a compute kernel (escape-time statistics over a grid of the complex
  plane). Hundreds of millions of operations per call and only numbers crossing the boundary:
  it becomes a WASM island automatically, with `escapeTime` compiled in as an internal call.
- `src/ease.ts`: a boundary-heavy helper. A few operations per call, called 100 000 times
  from a JS loop: every call into WASM would cost more than it computes, so it stays JS.

`src/main.ts` ties them together. Its `main()` prints the statistics; in a browser the
module renders them into the page.

```sh
pnpm dev             # Vite dev server (WASM is off in dev unless `dev.wasm` is set)
pnpm build           # dist/ with the kernel as a WASM island
pnpm explain         # trioxide explain --entry src/main.ts
pnpm compare         # time run() JS-only, auto, and with every eligible function in WASM
```

## What `trioxide explain` says

```
src/main.ts
  run (11:8): JS
    why: ineligible: returns values of different types (or a value on some paths only)
    failed rules:
      11:17  returns values of different types (or a value on some paths only)
      12:27  creates an array literal
      13:22  reads `VIEWS`, which is defined outside the function (only module-level constants can be read)
  main (23:8): JS
    why: ineligible: calls `run`, which can't be compiled to WASM
    failed rules:
      24:22  calls `run`, which can't be compiled to WASM
      11:17  in `run`: returns values of different types (or a value on some paths only)
      12:27  in `run`: creates an array literal
      13:22  in `run`: reads `VIEWS`, which is defined outside the function (only module-level constants can be read)

src/ease.ts
  ease (5:8): JS
    why: the cost model keeps it in JS: benefit − cost = -1900000 ≤ threshold 0
    cost model: benefit 0 − cost 1900000 = -1900000 ≤ threshold 0
      benefit: 600000 operations over 100000 call(s)
      cost: crossings 1200000 + marshalling 0 (0 bytes) + instantiation 700000
    passes:
      fold skipped at 6:13, 6:22: the call's result depends on what its callers pass (`passes.specialize` is off)

src/kernel.ts
  escapeTotal (6:8): WASM in island `kernel`
    why: chosen by the cost model; exported by the island
    cost model: benefit 1579034 − cost 700024 = 879010 > threshold 0
      benefit: 275867840 operations over 2 call(s)
      cost: crossings 24 + marshalling 0 (0 bytes) + instantiation 700000
    passes:
      inline skipped at 16:27, 17:41, 18:35, 19:55, 19:68: the binding's single value depends on what its callers pass (`passes.specialize` is off)
      fold skipped at 16:27, 17:41, 18:35, 19:55, 19:68: the expression's single value depends on what its callers pass (`passes.specialize` is off)
  escapeTime (26:8): WASM in island `kernel`
    why: compiled into the island as a callee (its JS version is kept too)
    cost model: benefit 1579034 − cost 700024 = 879010 > threshold 0
      benefit: 275867840 operations over 2 call(s)
      cost: crossings 24 + marshalling 0 (0 bytes) + instantiation 700000
    passes:
      inline skipped at 30:36: the binding's single value depends on what its callers pass (`passes.specialize` is off)
      fold skipped at 30:36: the expression's single value depends on what its callers pass (`passes.specialize` is off)

src/view.ts
  render (4:8): JS
    why: ineligible: can't prove what parameter `root` receives
    failed rules:
      4:24  can't prove what parameter `root` receives
      4:43  parameter `lines` is an array of anything, not of numbers
      5:16  uses the global `document` (a host API)
```

- `escapeTotal` becomes WASM: of its ≈275 million operations, the ≈40 million integer ones
  (counter increments and comparisons) are where the model expects WASM to save time
  (floating-point arithmetic is a tie), and that beats the fixed cost of instantiating the
  island. `escapeTime`
  joins the island as a callee; its JS version stays for the `ready()` fallback.
- `ease` stays JS: the model expects no saving on its floating-point arithmetic, while its
  100 000 calls would each pay a crossing.
- `run`, `main` and `render` are ineligible, and `explain` lists every rule each one fails
  with its position (strings, arrays of strings, module state, `document`).
- The `passes` lines are the JS passes' decisions inside each function: here, constant
  folding that would only hold for the values this program passes, which `passes.specialize`
  would allow.

## Same behaviour either way

The differential suite runs this example's `main()` types-stripped, as trioxide's JS, and with
the islands `auto` picks (`tests/differential`, legs a, b and d); all print the same. `pnpm
compare` times `run()` three ways (Node 22):

| build | islands | `run()` |
|---|---|---|
| JS only | — | 44.1 ms |
| auto | kernel | 46.8 ms |
| every cluster | ease, kernel | 49.0 ms |

The kernel runs about as fast as V8's optimised JS (scalar code is a tie today, see
`benchmarks/partition/README.md`), and moving `ease` too makes the program slower, as the
model predicts.
