# WASM lowering fixtures

One directory per lowering rule of the Rust backend (`crates/trioxide_rust`). Each has a
`kernel.ts` whose functions a `"use wasm"` directive forces into an island, and an `input.ts`
whose deterministic `main()` exercises them on edge values.

- The differential suite (`tests/differential`) runs every fixture as stripped JS, trioxide
  JS and JS+WASM; the outputs must match exactly.
- `cargo test -p trioxide_rust --test generated` snapshots the generated crate
  (`generated.snap` next to each fixture).

`tests/wasm-errors/` holds one fixture per eligibility rule: a forced function that breaks
it, with the diagnostic snapshotted in `diagnostics.snap`.
