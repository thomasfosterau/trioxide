# Benchmarks

Runtime benchmarks with [tinybench](https://github.com/tinylibs/tinybench): every case in
`cases/<name>/input.ts` exports a `bench()` that is timed twice, from its types-stripped JS
(`trioxide strip`) and from trioxide's optimised JS (`trioxide build --emit=js --no-wasm`,
app world), after checking both return the same value.

```sh
pnpm install
pnpm --filter trioxide-benchmarks bench            # builds trioxide_cli first
TRIOXIDE_BIN=target/debug/trioxide CASE=licm TIME=2000 pnpm --filter trioxide-benchmarks bench
```

The cases exercise the speed passes (PLAN §4): `array-pipelines` (pass 6), `for-of` (7),
`licm` (9) and `object-heavy` (10). The script prints a Markdown table of operations per
second and the speedup.
