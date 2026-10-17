# S3: Vite plugin, npm packaging, CLI distribution

Start from `main` after S1 has merged. Read `docs/sessions/common.md`, `docs/PLAN.md`
(especially D8, D9, D11, §7 "Vite plugin hooks" and "npm"), and `CLAUDE.md`. You are
building the M1 polish and most of M7. Other sessions will keep changing what the core
produces. Your job is the delivery surface around it, so code against the facade and napi
API only.

## You own
`packages/trioxide` (beyond S1's minimal loader), the `trioxide/vite` plugin, the npm
platform packages, `crates/trioxide_napi` (additive changes), the CLI ergonomics in
`crates/trioxide_cli`, release workflows, and `examples/dom-app`.

## Deliverables
1. **Vite plugin (`trioxide/vite`)** for Vite 8+:
   - `enforce: 'pre'` on `.ts/.tsx/.mts/.cts`; make sure Vite's own oxc transform doesn't
     process these files a second time.
   - `configResolved` merges plugin options with `trioxide.json`. Builds default to
     `world: "app"`.
   - `buildStart` creates the napi `Project` from Rollup inputs and runs whole-program
     analysis. `transform` serves `ModuleOutput` with sourcemaps.
   - Virtual modules `\0trioxide:island/<id>`: resolve and load the island glue. The glue
     content comes from `BuildOutput`; S7 produces real glue, so test with a fake island.
   - `generateBundle` emits `.wasm` assets with hashed names and rewrites glue references.
   - Dev server: `Project.update` on file change, invalidate dependants, HMR works.
     WASM is off unless `dev.wasm: true`.
   - Diagnostics surface through Vite's logger and error overlay with code frames.
2. **npm distribution** (the oxlint/rolldown pattern):
   - `@trioxide/binding-<triple>` optional dependencies for linux x64/arm64 (gnu + musl),
     darwin x64/arm64 and win32 x64.
   - The `trioxide` bin calls the binding's `runCli(argv)`, so there is one implementation.
   - Generated TypeScript types for the JS API.
3. **Standalone binary:** a GitHub Actions release workflow that cross-builds `trioxide`
   for the same triples, attaches the binaries plus checksums to GitHub releases, and
   publishes the npm packages on tag. Make publish a dry run unless the secrets exist.
   Document `cargo install --git`.
4. **CLI ergonomics:**
   - `trioxide check-toolchain` reports rustc, cargo, the wasm32 target and `wasm-opt`,
     and what's missing. It shares its detection logic with S7's driver via a small module
     in `trioxide_wasm`; coordinate through an additive API.
   - Coloured diagnostics, `--watch` for `build`, and config discovery up the directory
     tree.
5. **Example and tests:**
   - `examples/dom-app`: a small Vite 8 + TS DOM app with no WASM candidates.
   - Integration tests (vitest) that build it with and without the plugin and compare the
     runtime behaviour (Playwright with the preinstalled Chromium; never run
     `playwright install`).
   - A plugin unit test for the virtual-module, asset-emission and HMR paths using a fake
     island.

## Acceptance
- `pnpm -r test` passes, including the dom-app build-and-run comparison and the dev-server
  HMR test.
- The release workflow passes as a dry run in CI.
- A fresh Vite 8 app can use the plugin from the local package via a `file:` dependency.
  Document the steps in `packages/trioxide/README.md`.

## Out of scope
Optimisation logic, the Rust backend, and the real WASM glue (S7). Partition logic (S8).
