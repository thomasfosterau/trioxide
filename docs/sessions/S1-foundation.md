# S1: Foundation (runs alone, before everything else)

Start from branch `claude/implementation-plan`, which holds `docs/PLAN.md` and these
prompts; your PR merges them into `main` along with your work. Read
`docs/sessions/common.md` and all of `docs/PLAN.md` first. You are building milestones M0
and the core of M1. Every later session builds on the interfaces you set, so choose them
carefully and document them.

## You own
The whole workspace for now: root `Cargo.toml`, `rust-toolchain.toml`, CI, every crate
skeleton listed in PLAN §2 "Workspace layout", `crates/trioxide` (facade),
`crates/trioxide_project`, `crates/trioxide_cli`, `crates/trioxide_napi`, `packages/trioxide`
(minimal), and `tests/` harnesses.

## Deliverables
1. **Workspace and toolchain.**
   - Cargo workspace with shared lints (clippy pedantic where reasonable) and every crate
     from PLAN §2 created.
   - `rust-toolchain.toml` pinned to 1.99.1 with the `wasm32-unknown-unknown` target. If
     1.99.1 can't be installed here, record the exact failure in the PR and use the newest
     stable that can be.
   - `oxc_*` crates at `=0.154.0`.
   - tsrs added as a git dependency on `https://github.com/oxc-project/tsrs` pinned to a
     specific commit (`rev = …`), behind a default-on `tsrs` Cargo feature, used only from
     `crates/trioxide_types`.
   - Prove oxc and tsrs build together: a smoke test in `trioxide_types` that builds a tsrs
     program from an in-memory `tsrs_vfs` filesystem and prints the type of one identifier.
     Note any dependency conflicts and how you resolved them.
2. **Interfaces (stubs with doc comments).** Implement PLAN §7 in `crates/trioxide`:
   `Options` (serde, with a JSON schema generated via `schemars` and written to
   `schema/trioxide.schema.json`), `Project`, `ModuleOutput`, `BuildOutput`, `Report`,
   `Diagnostic` (with spans; use `miette` or oxc's diagnostics).
   Stub the cross-stream seams so wave-2 sessions can fill them without touching the facade:
   - `trioxide_types::TypeOracle`, a trait plus a no-op implementation. S2 implements it.
   - `trioxide_analysis::Facts`, the side tables keyed by `(ModuleId, NodeId/SymbolId)`
     with a trust level per fact. S2 fills them.
   - `trioxide_js::PassManager` with a `Pass` trait. S4/S5 add passes.
   - `trioxide_partition::Partitioner`, which returns "all JS" for now.
   - `trioxide_wasm::IslandSpec` (exported functions with ABI-level signatures) and an
     `IslandBuilder` trait. S7 implements them, S6 produces `IslandSpec`s.
3. **Passthrough pipeline.**
   - `trioxide_project`: module graph from entries using `oxc_resolver` (honouring
     `tsconfig.json` `paths`), parsing with oxc, and a content-hash cache.
   - The pipeline runs parse → (no-op analysis) → `oxc_transformer` TS strip (respecting
     `tsconfig` `target`/`jsx`) → `oxc_codegen` with sourcemaps.
4. **CLI.**
   - `trioxide transform <file>` (stdout) and `trioxide build <entries…> --out-dir`.
   - `--config`, `--emit=js`, and the flags from PLAN §7, parsed but marked as not yet
     implemented where that's the case.
   - Reads `trioxide.json` / `.jsonc`.
5. **napi binding and npm package.**
   - `crates/trioxide_napi` (napi-rs v3) exposing `transform(code, filename, options)` and
     a `Project` class (`build`, `update`, `transform`).
   - `packages/trioxide` loads the local build and has a typed JS API. Platform packages
     and the Vite plugin are S3's; leave a `vite` subpath export stub.
   - pnpm workspace at the root.
6. **Test harnesses.**
   - `tests/fixtures/<case>/input.ts` runs through the pipeline with insta snapshots,
     auto-discovered.
   - `tests/differential/`: a Node runner that executes each fixture's `main()` three ways:
     (a) types stripped only (plain `oxc_transformer`), (b) `trioxide --emit=js`,
     (c) JS+WASM, skipped until S7. It asserts identical stdout and runs under
     `cargo test` or `pnpm test`.
   - Seed about 10 fixtures covering classes, enums, generics, modules and imports, async,
     and JSX.
7. **CI.** GitHub Actions on Linux: fmt, clippy, cargo test, pnpm build + test,
   differential suite. Cache cargo and pnpm. Install the wasm32 target.
8. **Docs.**
   - Root `README.md` with the project pitch, build instructions and status.
   - `CLAUDE.md` with workspace conventions, commands, crate ownership per stream (from
     `docs/sessions/README.md`), and the pin policy.

## Acceptance
- CI is green.
- `cargo run -p trioxide_cli -- transform tests/fixtures/<any>/input.ts` prints JS that
  matches plain type-stripping.
- The differential suite passes on all seed fixtures.
- The napi binding builds, and a Node test calls `transform`.
- The tsrs smoke test passes.

## Out of scope
Real analysis, optimisation passes, Rust/WASM generation, the Vite plugin, release
packaging.
