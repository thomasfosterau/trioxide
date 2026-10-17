# Trioxide implementation plan

Trioxide is a TypeScript-aware optimising compiler built on the oxc toolchain. It takes a
TypeScript project and, per function, either emits **optimised, readable JavaScript** or
transpiles it to **Rust and compiles that to WebAssembly**, wiring the two together with
generated glue. It ships as a **standalone binary** and as a **Vite plugin**.

It is not a minifier. Output stays readable (names, structure and comments preserved where
possible); size reduction is left to the bundler's minifier. The optimisations it shares with
minifiers (constant folding, DCE) are there to enable the ones that matter for speed.

---

## 1. Design decisions

These shape everything else. All of them are agreed.

| # | Decision | Choice |
|---|----------|--------|
| D1 | **How far to trust TS types** | TS types are unsound (`any`, `as`, bivariance, unchecked indexing), even when the checker computes them correctly. Every fact has a trust level: `Proven` (from literals, flow, or a TS type with no unsound construct feeding it), `Declared` (a TS type that rests on an annotation, cast, `any` or an open-world boundary), or `Unknown`. By default, passes that change semantics when wrong need `Proven`. `trustTypes: true` lets `Declared` count as proof. |
| D2 | **World assumption** | `app` mode (closed world: the whole module graph is visible and exports are only used internally, as in a Vite app build) vs `library` mode (open world: exported signatures are `Declared` at best). Vite builds default to `app`; the CLI defaults to `library` unless entries are given. |
| D3 | **Type checking via tsrs** | [tsrs](https://github.com/oxc-project/tsrs) (the oxc project's Rust port of the TypeScript 7 checker) is embedded in-process as the source of TS types: types at locations, symbols, resolved signatures, flags. It sits behind the `trioxide_types` adapter crate so its API never leaks. Trioxide does not reimplement TS inference; its own analysis adds only what tsrs lacks (ranges, integer-ness, effects, escape, shapes). Reporting TS errors is opt-in (`check: true`). tsrs is behind a default-on Cargo feature. |
| D4 | **Partition granularity** | Per function, grouped into **WASM islands**: connected clusters of eligible functions in the call graph. One island becomes one Rust crate and one `.wasm` module. |
| D5 | **Every island keeps a JS fallback** | The optimised-JS version of each island function is always produced. It is used in dev mode, when the Rust toolchain is missing (with a warning), and as the oracle for differential tests. |
| D6 | **Rust toolchain at build time** | Compiling islands shells out to `cargo build --target wasm32-unknown-unknown --release`, with outputs cached by content hash. Users need `rustup` + the wasm32 target. The Rust IR stays backend-agnostic so a direct `wasm-encoder` backend (no toolchain) could be added later; that is not v1 work. |
| D7 | **Own JS↔WASM ABI, not wasm-bindgen** | The compilable subset is narrow (numbers, booleans, typed arrays, number arrays, plain-data structs). A small generated glue layer costs less and avoids pinning `wasm-bindgen-cli` versions. `wasm-opt` runs if it is on PATH. |
| D8 | **Whole-program core, per-file facade** | The Rust core owns a `Project` (module graph via `oxc_resolver`, incremental cache keyed by content hash). Vite's per-file `transform` hook asks the project for already-computed results. In dev, analysis is incremental and WASM is off by default. |
| D9 | **Targets Vite 8+ (Rolldown/oxc-based)** | The plugin runs `enforce: 'pre'` on `.ts/.tsx/.mts/.cts`, so it sees types before Vite's own oxc transform strips them, and returns JS. |
| D10 | **Pin oxc and tsrs exactly** | All `oxc_*` crates pinned to one exact version (currently `=0.154.0`) and bumped together in dedicated PRs. `oxc_resolver` and `oxc_sourcemap` live in separate repositories with their own version lines; they are pinned exactly too (`oxc_sourcemap` to the version `oxc_codegen` uses). tsrs is not on crates.io, so it is a git dependency pinned to a commit and bumped the same way. Both make breaking changes often. The workspace pins Rust 1.99.1 in `rust-toolchain.toml` (tsrs's MSRV). tsrs pins `unicode-id-start =1.1.2` (Unicode 15.1, matching TypeScript), and oxc resolves to the same version. |
| D11 | **Config** | `trioxide.json` / `trioxide.jsonc` with a JSON schema, merged with Vite plugin options. `tsconfig.json` is read for `paths`, `target` and `jsx`. Source directives override everything: `"use wasm"` / `"use js"` at the top of a file or function body, or a `/** @trioxide wasm|js */` JSDoc tag. |

---

## 2. Architecture

```
            ┌──────────── trioxide CLI (clap) ─────────────┐
            │                                              │
 Vite ──► napi binding ──► trioxide (facade crate: Project, pipeline API, options)
                               │
     ┌────────────┬────────────┬────────────┼───────────────┬──────────────────┐
  project       types       analysis    partition        js backend        rust backend ──► wasm driver
 (graph,        (tsrs       (effects,   (eligibility,    (oxc_traverse     (subset check,     (cargo, cache,
  resolver,      adapter,    call graph, cost model,      optimisation      lowering to RIR,   wasm-opt,
  cache)         span map,   ranges,     islands)         passes, codegen)  Rust emission)     JS glue)
                 soundness)  escape)
```

### Pipeline (per build)

1. **Load**: resolve entries, build the module graph, parse every module with `oxc_parser`
   (TS kept), and build `oxc_semantic` per module.
2. **Type-check**: build a tsrs program over the same sources (in-memory VFS, the same
   `tsconfig.json`). Walk each oxc AST, match nodes to tsrs nodes by byte span, query the
   nodes trioxide cares about (identifiers, member and call expressions, parameters,
   returns), and convert the answers into trioxide types (§3). With `check: true`, also
   report tsrs diagnostics.
3. **Link**: resolve imports to exports across modules, producing global `SymbolRef`s
   (module id + `SymbolId`).
4. **Analyse**, as side tables keyed by `(ModuleId, NodeId/SymbolId)`:
   - call graph, including resolved method calls on known classes
   - effect and purity summaries per function (reads/writes/allocates/throws/calls unknown)
   - soundness refinement of tsrs types into `Proven` / `Declared` (§3)
   - numeric range and integer analysis
   - escape analysis for object literals and class instances
   - builtin models: a hand-written table of types and effects for `Math.*`,
     `Array.prototype.*`, typed arrays, `String.prototype.*`, `Number.*`, `JSON`, etc.
5. **Partition**: mark eligible functions, cluster them into islands, score each island with
   the cost model, and keep the profitable ones (§5).
6. **Emit JS**: run the optimisation passes (§4) on every module, then strip the TS with
   `oxc_transformer`, replace island functions with imports from the island's glue module,
   and codegen with `oxc_codegen`, sourcemaps included.
7. **Emit WASM**: for each island, lower to RIR, emit a Rust crate, run `cargo build`
   (cached), optionally `wasm-opt`, and generate the JS glue.
8. **Report**: write a machine-readable decision log for `trioxide explain`: per function,
   JS or WASM, why, and which passes fired.

Passes that rewrite the AST invalidate semantic data. The pass manager rebuilds
`oxc_semantic` and the affected analysis tables between pass groups, the same way the oxc
minifier iterates to a fixed point. Analyses must be cheap to recompute per module.

### Workspace layout

```
Cargo.toml                    # workspace, exact oxc pins + tsrs git pin, shared lints
rust-toolchain.toml           # 1.99.1
crates/
  trioxide/                   # facade: Project, Options, build(), transform(), strip_types()
  trioxide_common/            # ids (ModuleId, NodeRef, SymbolRef), Diagnostic, decision-log records
  trioxide_project/           # module graph, oxc_resolver, tsconfig, content-hash cache
  trioxide_types/             # the only crate that touches tsrs: program setup, span map, type conversion
  trioxide_analysis/          # linker, call graph, effects, soundness, ranges, escape, builtins
  trioxide_js/                # JS optimisation passes + pass manager
  trioxide_partition/         # eligibility, islands, cost model, directives
  trioxide_rust/              # subset checker, RIR, Rust source emitter
  trioxide_wasm/              # cargo driver, build cache, wasm-opt, JS glue generator
  trioxide_rt/                # runtime crate the generated Rust depends on (JS-compatible math, arrays, ABI helpers)
  trioxide_cli/               # `trioxide` binary
  trioxide_napi/              # napi-rs v3 binding
packages/
  trioxide/                   # npm: binding loader, JS API, `trioxide/vite` plugin, CLI shim
tests/
  fixtures/                   # <case>/input.ts (+ trioxide.json) → snapshots
  differential/               # runs original vs optimised-JS vs WASM in Node and compares output
examples/
  dom-app/                    # should stay JS
  image-filters/              # should become WASM
  mixed/                      # both, with a boundary
benchmarks/
docs/
```

---

## 3. Type system ("trioxide types")

TS types come from **tsrs**. Trioxide does not infer them itself. `trioxide_types` converts
each tsrs answer into trioxide's own lattice, so no tsrs handle (`P<Type>`, `P<Symbol>`)
escapes the adapter:

- `Number { range: Interval, int: bool, nonNaN: bool, nonNegZero: bool }`, where tsrs
  supplies `number` and numeric literal types and trioxide's range analysis fills in the
  rest
- `Boolean`, `String`, `BigInt`, `Symbol`, `Null`, `Undefined`
- `Array(elem)`, `TypedArray(kind)`, `Tuple([...])`, identified by comparing against tsrs's
  global types (`Array`, `Float64Array`, …)
- `Object(Shape)`: known keys with their types. A shape is *exact* (exactly these own data
  properties, `Object.prototype` as prototype) only when the analysis proves it from the
  object literal's creation and every use; TS object types are open, so oracle shapes never
  are
- `Instance(ClassId)` for classes with known fields and no dynamic mutation
- `Function(Signature, FunctionId?)` from resolved signatures
- `Union([...])`, `Unknown` (top), `Never`

**Adapter (`trioxide_types`).** It wraps the handful of queries trioxide uses:
`get_type_at_location`, symbol at location, `get_resolved_signature`, type of a symbol, and
`Type::flags`. It owns program setup (an in-memory `tsrs_vfs`, `tsconfig.json` parsing,
bundled `lib.d.ts`) and incremental updates (`update_program`). It is also the only place
that knows the tsrs API, so tsrs bumps stay local.

The VFS is an overlay: the project's modules come from memory (so unsaved Vite edits are
seen), everything else (`tsconfig.json`, `node_modules` declarations) from disk. Every module
in the graph is added to the tsconfig's root files. Without a tsconfig the oracle uses strict,
`esnext`, `bundler`-resolution defaults. Conversion is conservative: `any`, `unknown`, `void`
(a `void` function may return anything), intersections, library classes and index-signature
objects become `Unknown`. `Instance` and `Function` answers name oxc nodes, so the analysis
registers its class and function declarations with the oracle (`register_declarations`)
before asking.

**Span correlation.** oxc and tsrs both use UTF-8 byte offsets. For each oxc node of
interest, the adapter finds the tsrs node with the same start (skipping TS-style leading
trivia via `get_start_of_node`) and end. Parenthesised expressions and JSDoc reparsing
differ between the two ASTs, so this mapping gets dedicated tests. Unmatched nodes become
`Unknown`, never a guess.

**Soundness (trust levels, D1).** A tsrs type is `Proven` only when nothing unsound feeds
it:
- it is derived from literals, operators and builtin models, and flow narrowing (`typeof`,
  `===`, `instanceof`, truthiness), through bindings that are not reassigned with
  weaker-typed values
- no `any`, `as` cast, `!`, unchecked index access, or call to an unknown function lies on
  the path
- function parameters are `Proven` only when every call site is known (closed world) and
  every argument is `Proven` at that type; otherwise the parameter is `Declared`

Reaching `eval`, `with`, `arguments` aliasing, `Proxy`, or `Object.defineProperty` on the
value degrades the affected bindings to `Unknown`. The bottom-up interprocedural summary pass
over the call graph's SCCs propagates trust levels together with effects.

**Embedding constraints.** tsrs checkers take `&mut self` (one per thread, behind a mutex),
and its arenas are `'static`. The oracle runs every type query on the program's first
checker and converts each answer before releasing it, so types from different checkers never
meet and no tsrs handle outlives a query. Programs are built with tsrs's checker and
file-loader regions, so the oracle can free a replaced program (`free_program`, then the
shared file data and regions, in the order tsrs's own watcher uses). Updates are applied
lazily with `update_program`; files it reparses still leak, so the adapter rebuilds from
scratch after a configurable number of updates (default 100) or above a resident-memory
threshold (default 4 GiB, Linux). If that proves insufficient, the checker moves to a worker
process. A tsrs panic during a query is caught, answered as "unknown", and forces a rebuild.

`plain-ptrs` (plain 64-bit handles instead of compressed 32-bit ones, avoiding the 32 GiB
address reservation) is **not** enabled by default. Measured with
`examples/oracle_memory.rs` (40k and 204k lines, release build): compressed handles use 16–20%
less resident memory (68 vs 81 MiB, 237 vs 295 MiB) at the same speed; plain handles only
save virtual address space (1.7 vs 34 GiB reserved). It is available as the `plain-ptrs`
feature of `trioxide_types` for environments that cap virtual memory.

---

## 4. JavaScript optimisation passes

Each pass is a separate `oxc_traverse` visitor with its own option flag. Each must come with
snapshot tests and differential execution tests. Constant evaluation and side-effect queries
reuse `oxc_ecmascript`; trioxide does not run the oxc minifier.

**Foundation** (also feed the WASM path):
1. Cross-module constant and `enum` / `const enum` inlining.
2. Constant propagation and folding, using range facts.
3. Dead code elimination: unreachable branches, unused pure bindings, unused exports in
   `app` mode.
4. Non-null / `typeof` check elimination where types prove the outcome (`?.`, `??`, guards).

**Speed-oriented** (what separates trioxide from a minifier):
5. Function inlining for small, pure or single-call-site functions, with a size budget.
6. Lowering array pipelines to loops: `arr.map(f).filter(g).reduce(h)` becomes one fused
   `for` loop when the receiver is a proven `Array` and the callbacks are inlineable.
7. `for…of` over proven arrays becomes indexed loops; spread of proven arrays is lowered.
8. Scalar replacement: non-escaping object literals and tuples become locals
   (`const p = {x, y}; p.x + p.y` → `x + y`).
9. Loop-invariant code motion for pure expressions (including `.length` when the array is
   not mutated in the loop).
10. Hidden-class stabilisation: initialise all class fields in the constructor in a
    consistent order, avoid `delete` on owned shapes, and keep array element kinds
    monomorphic where provable.
11. Compile-time partial evaluation: pure top-level calls with constant arguments
    (`const TABLE = buildTable()`) are evaluated by a fuel-bounded interpreter over the
    analysed subset and replaced with literals when the result is small enough.
12. Closure hoisting: closures that capture nothing are lifted out of hot loops and
    functions.

Every pass has a pre-condition expressed in analysis facts. A pass whose facts are only
`Declared` is skipped unless `trustTypes` is on, and the skip is logged for `explain`.

---

## 5. Partitioning (JS vs WASM)

**Eligibility.** A function can become WASM only if its body is in the compilable subset:
- parameters and return types are numbers, booleans, typed arrays, `number[]`, or plain-data
  classes and objects that do not escape across the boundary
- the body uses only local control flow, arithmetic, `Math.*`, typed array and array
  indexing, loops, calls to other eligible functions, and non-escaping struct-like
  classes/objects
- no DOM or host APIs, no closures capturing JS objects, no dynamic property keys, no
  exceptions crossing the boundary, no `async`/generators, and (in v1) no strings

**Clustering.** Take connected components of eligible functions in the call graph. Every
call edge into a component from JS code is a boundary entry.

**Cost model** (per island, deliberately simple at first and tuned with benchmarks):
- *benefit*: estimated compute weight, i.e. loop nesting × arithmetic/indexing op density,
  higher for proven-integer maths and typed-array work
- *cost*: boundary crossings (calls into the island from loops weigh heavily), marshalling
  bytes (typed arrays copied in/out of linear memory, `number[]` converted), plus a fixed
  per-island instantiation cost
- an island is kept if `benefit − cost > threshold`; otherwise it stays JS (and still gets
  the JS passes)

**Overrides.** `"use wasm"` forces an island; it is an error if the function is ineligible,
and the diagnostic explains why. `"use js"` forbids one. Config globs do the same per file.
`mode: "js-only" | "auto" | "wasm-preferred"`.

**Explainability.** `trioxide explain [file[:fn]]` prints each decision, the failed
eligibility rule with its source span, and the cost-model numbers. This is how users come to
trust the partitioner.

---

## 6. Rust backend and WASM

**RIR (Rust IR).** A small typed tree IR: functions, structs, `f64`/`i32`/`u32`/`bool`
locals, slices into linear memory, loops, and calls. It is lowered from the oxc AST plus
analysis side tables. The emitter prints Rust source (via `prettyplease`) into
`<cache>/islands/<hash>/src/lib.rs`, plus a `Cargo.toml` that depends on `trioxide_rt`.

**Semantic fidelity rules.** The differential tests enforce these:
- `number` → `f64` by default; `i32` only where range analysis proves the value is an
  integer and that no operation overflows (e.g. `x | 0`, loop counters bounded by `.length`)
- JS `%` on f64 matches Rust's `%`; `Math.round`, `Math.sign`, `Math.max/min` (NaN, -0) get
  JS-exact implementations in `trioxide_rt`; transcendental functions use `libm` (the spec
  allows implementation-approximated results, but this is documented)
- bitwise ops apply ToInt32/ToUint32 explicitly
- array indexing: `Proven` in-bounds accesses are unchecked; anything else uses a checked
  helper that returns `NaN` for out-of-bounds numeric reads (JS's `undefined` coerced in
  arithmetic). An unprovable pattern where OOB would change semantics makes the function
  ineligible
- integer division and NaN comparisons follow JS

**ABI and glue.** Numbers and booleans pass directly. Typed arrays and `number[]` are copied
into linear memory through an exported bump allocator in `trioxide_rt`. Copy-back happens
only when the analysis shows the island mutates the array. Each island gets a glue ESM
module that instantiates the `.wasm` (top-level await by default, or a `ready()` promise when
the target lacks TLA) and exports JS wrappers with the original signatures.

**Build driver.** Cache key = hash(RIR, `trioxide_rt` version, rustc version, profile).
Islands build in parallel. A missing toolchain falls back to JS with one clear warning, plus
an `--require-wasm` flag that turns it into an error.

---

## 7. Interfaces

**Rust facade (`trioxide`):**
```rust
pub struct Options { /* mode, world, trust_types, check, tsconfig, target, sourcemap, passes, wasm, dev, cache_dir, overrides */ }
pub struct Project { /* graph + caches */ }
impl Project {
    pub fn new(root: PathBuf, options: Options) -> Result<Self>;
    pub fn build(&mut self, entries: &[PathBuf]) -> Result<BuildOutput>;      // whole program
    pub fn update(&mut self, path: &Path, source: Option<String>) -> Result<Invalidation>;
    pub fn transform(&mut self, path: &Path) -> Result<ModuleOutput>;        // per-file, Vite
    pub fn explain(&self, query: &ExplainQuery) -> Report;
}
pub fn strip_types(root: &Path, path: &Path, source: &str, options: &Options) -> Result<ModuleOutput>; // differential baseline
pub struct ModuleOutput { path: PathBuf, code: String, map: Option<SourceMap /* v3 JSON */>, islands: Vec<IslandRef>, diagnostics: Vec<Diagnostic> }
pub struct BuildOutput { modules: Vec<ModuleOutput>, islands: Vec<IslandArtifact /* wasm bytes + glue */>, report: Report }
```

Problems in user code are `Diagnostic`s on the outputs (plain, serialisable data with
spans, rendered with `oxc_diagnostics`); `Err` is reserved for I/O, invalid config and
unreadable entries. The `Options` JSON schema is generated into
`schema/trioxide.schema.json`.

**Cross-stream seams** (S1 stubs, filled by later streams):
`trioxide_types::TypeOracle` (+ `NoopTypeOracle`), `trioxide_analysis::Facts` (tables keyed
by `NodeRef`/`SymbolRef`, each fact with a `Trust`), `trioxide_js::{Pass, PassManager,
FactSource}`, `trioxide_partition::Partitioner`, `trioxide_wasm::{IslandSpec,
IslandBuilder}`.

**CLI:** `trioxide build [entries…] --out-dir dist [--watch]`, `trioxide transform <file>`
(stdout), `trioxide explain [target]`, `trioxide check-toolchain [--json]`. `--config`,
`--root`, `--mode`, `--world`, `--trust-types`, `--no-wasm`, `--emit=js|rust|wasm|report`,
`--color=auto|always|never`. A hidden `trioxide strip <file>` prints types-stripped-only output
for the differential suite. The CLI is the `trioxide_cli` library's `run(argv)`, which both the
binary and the napi binding's `runCli` call. Without `--config`, the config file is the nearest
`trioxide.json(c)` at or above the project root, stopping at the repository root (`.git`).
`--watch` polls modification times of every module in the graph plus the config and tsconfig.
`check-toolchain` uses `trioxide_wasm::toolchain`, the same probe the island builder uses.

**npm:** `import { transform, build, explain } from 'trioxide'` and
`import trioxide from 'trioxide/vite'`. The `trioxide` npm bin calls into the napi binding,
so there is one implementation (the oxlint/rolldown pattern). Prebuilt platform binaries
ship as `@trioxide/binding-<platform>` optional dependencies, using napi-rs platform names
(`linux-x64-gnu`, `linux-arm64-gnu`, `linux-x64-musl`, `linux-arm64-musl`, `darwin-x64`,
`darwin-arm64`, `win32-x64-msvc`) rather than Rust triples. The options type
(`TrioxideOptions`) is generated from the JSON schema, so it cannot drift from `Options`.

**Vite plugin hooks:** `config` (exclude `.ts/.tsx/.mts/.cts` from Vite's oxc transform, so
trioxide's JS is not re-parsed as TypeScript), `configResolved` (merge `trioxide.json` with
the plugin options, decide app/library: `library` only for `build.lib`), `buildStart`
(create the Project and run whole-program analysis from Rollup inputs, taking the module
scripts of HTML inputs as entries), `resolveId`/`load` for `\0trioxide:island/<id>` glue
modules, `transform` (serve `ModuleOutput`, `moduleType: 'js'`), `generateBundle` (emit
`.wasm` assets with hashed names and point the glue at them), `hotUpdate` (Vite 6+'s
replacement for `handleHotUpdate`: `Project::update`, then invalidate the dependants whose
output actually changed, so ordinary edits keep Vite's normal HMR propagation). In dev, WASM
is off unless `dev.wasm: true`; dev glue gets its `.wasm` inlined as a `data:` URL, which
Vite's `new URL(…, import.meta.url)` handling leaves alone.

**Glue ↔ emitter contract:** glue refers to its `.wasm` as
`new URL(WASM_URL_PLACEHOLDER, import.meta.url)` (`"__TRIOXIDE_WASM_URL__"`,
`trioxide_wasm::WASM_URL_PLACEHOLDER`); whoever emits the glue replaces the placeholder with the
`.wasm`'s URL relative to the glue.

---

## 8. Testing strategy

Parallel sessions rely on this to stay safe.

- **Snapshot tests** (`insta`): `tests/fixtures/<case>/input.ts` → `output.snap` (every
  module of the build), later also `island_*.rs` and `report.json`. Every pass and lowering
  rule needs fixtures.
- **Differential execution** (the core correctness gate): each fixture exports a `main()`
  that prints results. A Node runner executes three variants (types stripped only,
  trioxide JS, trioxide JS+WASM) and asserts identical output. CI fails on any divergence.
- **Generated programs**: a small generator of random programs in the numeric subset
  (expressions, loops, typed arrays), fed through the differential runner, to fuzz the Rust
  backend's semantics.
- **Conformance smoke**: run the JS passes over a corpus of real TS projects (e.g. vendored
  snapshots of a few libraries) and check they still parse, typecheck-strip, and pass their
  own tests where cheap.
- **Benchmarks**: `benchmarks/` with tinybench (runtime of outputs) and criterion (compiler
  throughput). Cost-model tuning is driven by these numbers.
- **Every PR**: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `pnpm test`,
  and the differential suite. The WASM jobs need `rustup target add wasm32-unknown-unknown`.

---

## 9. Milestones

| M | Scope | Exit criteria |
|---|-------|---------------|
| M0 | Workspace scaffold, CI, lints, Rust 1.99.1 pin, crate skeletons with the §7 interfaces stubbed, oxc + tsrs building together, fixture + differential harness | CI green; `trioxide transform` on a TS file prints types-stripped JS |
| M1 | Passthrough pipeline end to end: Project, module graph, napi binding, npm package, Vite plugin returning stripped JS with sourcemaps | `examples/dom-app` builds with the plugin, and the output matches plain Vite |
| M2 | `trioxide_types` (tsrs adapter, span map, type conversion, optional `check`), then analysis: linker, call graph, effects, soundness refinement, builtin models, ranges, escape | Span-mapping tests; analysis snapshot tests (facts printed per node) for ~40 fixtures |
| M3 | JS passes 1–4 (foundation) | Differential suite green; measurable wins on fixtures |
| M4 | JS passes 5–12 | Benchmarks show speedups on array-pipeline and object-heavy fixtures |
| M5 | Rust backend: subset checker, RIR, emitter, `trioxide_rt`, ABI, glue, cargo driver + cache | `"use wasm"` functions in `examples/image-filters` run as WASM and match JS output exactly |
| M6 | Partitioner, cost model, directives, `explain` | `examples/mixed` auto-partitions sensibly; `explain` output reviewed |
| M7 | Vite polish (HMR, dev mode, asset emission), binary distribution (GitHub releases + npm platform packages), docs | `npm i -D trioxide` + plugin works on a fresh Vite 8 app |

### Work streams and dependencies (for parallel sessions)

```
S1 Foundation (M0+M1 core) ──┬──► S2 Analysis (M2) ──┬──► S4 JS foundation passes (M3) ──► S5 JS speed passes (M4, can split in 2)
                             │                       ├──► S6 Rust subset + RIR + emitter ──┐
                             │                       │                                      ├──► S8 Partitioner + explain (M6) ──► S9 Integration, examples, benchmarks
                             ├──► S3 Vite plugin + npm + CLI distribution (M1 polish, M7)   │
                             └──► S7 trioxide_rt + ABI + glue + cargo driver (hand-written Rust first) ─┘
```

S2 starts with `trioxide_types`, the tsrs adapter, which the rest of the analysis consumes.
S1 must land first and alone because it fixes the interfaces. S2, S3 and S7 can then run in
parallel. S4 and S6 can run in parallel once S2's fact tables exist. S7 can define the ABI
before S6 is done, using hand-written island crates.

---

## 10. Risks

- **oxc churn**: exact pins, a single upgrade owner per bump, and analysis kept behind our
  own side tables so AST changes stay local.
- **tsrs churn and packaging**: it is not published, has no API stability promise, and
  releases almost daily, and upstream does not take issues or PRs. Mitigations: a git
  commit pin, the `trioxide_types` adapter as the only consumer, and a Cargo feature so
  trioxide still builds (with `Declared`/`Unknown`-only facts) without it.
- **Double parsing and memory**: every file is parsed by both oxc and tsrs, and tsrs leaks
  its arenas. This is acceptable for builds; dev mode uses periodic program rebuilds or a
  worker process (§3).
- **Unsound types leading to miscompiles**: the trust levels (D1) plus the differential
  suite; `trustTypes` stays opt-in.
- **Boundary overhead erasing WASM wins**: the cost model is conservative by default and
  `explain` shows the numbers.
- **Rust toolchain friction**: JS fallback (D5), `check-toolchain`, and a later
  direct-to-wasm backend kept possible by the RIR design.
- **Scope creep in the subset** (strings, closures, exceptions): v1 keeps a narrow numeric
  subset. Strings and handle-based object interop come after v1.
