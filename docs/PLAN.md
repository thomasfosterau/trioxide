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
| D2 | **World assumption** | `app` mode (closed world: the whole module graph is visible and exports are only used internally, as in a Vite app build — except the entry modules' exports and what they re-export, which the host calls) vs `library` mode (open world: exported signatures are `Declared` at best). Vite builds default to `app`; the CLI defaults to `library` unless entries are given. |
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
   `tsconfig.json`). Nodes are matched to tsrs nodes by byte span and queried on demand,
   during analysis, only where trioxide's own analysis knows nothing better (identifiers,
   member and call expressions, parameters); answers are converted into trioxide types (§3).
   With `check: true`, also report tsrs diagnostics.
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
  conformance/                # vendored TS libraries built by trioxide, run against their own tests
examples/
  dom-app/                    # should stay JS
  image-filters/              # filters the cost model scores (today's islands are slower at them, so they stay JS)
  mixed/                      # both, with a boundary
  telemetry/                  # a workload for the JS speed passes
benchmarks/                   # trioxide-benchmarks: one entry point and timing harness
  passes/                     # one case per JS speed pass
  programs/                   # whole programs (plus the examples), stripped vs trioxide vs auto + WASM
  partition/                  # cost-model calibration: JS vs WASM timings, fit, reach
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
`Unknown`, never a guess. The lookup walks down from the file through the children that
contain the span (a matching node always contains it) instead of indexing every node, since
a file is queried far less often than it has nodes; ties go to the node that most directly
denotes a value, then the innermost.

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

**How the analysis proves things (`trioxide_analysis`).** Rather than checking tsrs types
for unsound constructs after the fact, the analysis computes values itself and only falls
back to TS where it knows nothing:

- *Abstract interpretation.* Every unit (module body, function, arrow, static block, class
  field initialisers) is interpreted over abstract values `{ type, trust, points-to }`.
  Locals written only by their own unit are flow-sensitive (branches narrow and join, loops
  iterate to a fixed point with widening); everything else lives in whole-program tables
  that only grow by joins (symbols, parameters per position, `this`, returns, superclasses,
  `export default` values, and a flow-insensitive heap of allocation sites). A worklist
  re-interprets a unit only when a table entry it read changed. A value the analysis
  computed from literals, operators, builtin models and narrowing is `Proven`; where it
  knows nothing, TS's type at that node is used as `Declared`.
- *Openness instead of "is every caller known".* A function's parameters and `this` are
  joined with "unknown" (so at best `Declared`) exactly when its function object, class,
  prototype or an instance may reach code trioxide doesn't see: escape analysis over
  allocation sites, with library-mode exports, unknown callees, `eval`/`with` scopes,
  decorators, JSX factories and TS namespaces as escape points. In app mode an export alone
  does not escape, unless it is an entry module's (or re-exported by one): hosts, HTML module
  scripts and test runners call those. A site joined with an unknown object (so no longer
  followed) escapes too.
- *Poisoned values.* Values from `eval`, `Proxy`, `Reflect`/`Object.defineProperty` targets
  and similar reflection are `Unknown` *and* skip the TS fallback, since TS's type for them
  is not evidence.
- *Builtin integrity.* Builtin models assume code outside the program doesn't patch
  builtins, and check that the program doesn't. Any shadowing, assignment, or use of a
  namespace (`Math`, `JSON`, …), a typed array constructor or the global object other than a
  plain member read, call, construction or `instanceof` turns those models off; any way of
  reaching a prototype without naming it (`__proto__`, `getPrototypeOf`, `.constructor`,
  `X.prototype` of anything but a program class, `Reflect`, `Function`, `eval`) turns all of
  them off. Without integrity, method models, `Object.prototype` lookups, holes and iteration
  are `Unknown`. A key computed at run time that happens to be `"__proto__"` is not detected.
- *Temporal dead zones.* Reading a `let`/`const`/`class` binding before its declaration
  runs throws: in its own unit when the read comes first in the text (or in a `switch`), in
  another unit when the declaring unit may call out before the declaration, and across an
  import cycle: from another unit, any read of a module-level binding of a module in an
  import cycle may throw, since another module of the cycle may call in before the module's
  top level has declared it (`tdz-cycle`).
- *Convergence.* Counted per entry, not per round: an entry's numbers are widened once
  three merges have changed it (each property of a heap entry, and its elements, counted on
  their own), and an entry that more than 20 merges changed is given up (set to "any
  value", its objects escaping, its flags to "anything may happen" for good), so the run
  always reaches a fixed point and only facts depending on those entries are lost, with a
  warning. Counting per entry keeps both independent of how many sweeps the chunking takes.
  The 40-round "no facts at all" backstop is a last resort no program in the test corpus
  reaches (a test checks). Array lengths only go from unset to known to unknown, never
  back.
- *Effects* are computed twice, once counting only `Proven` facts and once counting
  `Declared` ones as proof, so `Facts::effects` honours `trustTypes`.
- *Ranges* use IEEE-exact interval arithmetic with `int`/`nonNaN`/`nonNegZero` flags;
  `i < a.length` bounds make `a[i]` an in-bounds fact while `a`'s length is unchanged.

Beyond types, the `Facts` API answers per call site (callees and whether that is all that
can run there), per expression (effects including callees and implicit calls, `is_pure`,
points-to sites), per loop (effects, written locals, arrays whose length the body may change,
induction ranges, invariant expressions), per unit (callers, reachability distinct from
`never`), and per declaration: `const` bindings with literal values (`symbol_constant`; the
type lattice has no string, boolean or bigint literals), enum members (`enum_member`:
`Proven` when nothing can change the enum object, `Declared` when only TypeScript's read-only
members say so) and exports no module can use (`export_used`, app world). For the speed passes
it also answers whether the program leaves a builtin and its prototype alone
(`builtin_intact`), whether an array allocation site never has holes (`may_have_holes`), and
whether any code can observe which own keys a class's instances have, or in which order
(`instance_keys_observed`: `Proven` false only for a class the analysis follows entirely, that
nothing extends, whose prototype gets nothing outside its body, and whose instances never reach
`for…in`, `in`, object spread or rest, or an `Object.*`/`JSON.*`/`Reflect.*`, `.call`/`.apply`,
`structuredClone`, `hasOwnProperty` or `propertyIsEnumerable` call).

Implicit calls count wherever the engine makes them: accessors (class ones, those of object
literals and of a literal's `__proto__`, and getters a spread reads), coercions, `await`,
iteration, and protocol methods (`[Symbol.iterator]`, `[Symbol.toPrimitive]`, `toJSON`, …),
which are open units whose callers are never complete. Whether user iterator code runs
never rests on declared types alone. Facts about code that never runs are vacuous, so
passes check `reachable` first; a loop's `resized` is only complete when
`resizes_untracked` is false, and an induction `step` is nominal (exact only for integers
within ±2^53).

**Refreshing facts.** Facts are keyed by `NodeId`s, which a rewrite invalidates. For v1 the
facade's `FactRefresher` (the `trioxide_js::FactSource` the pipeline runs passes with)
refreshes them for a module that a pass group changed, when the next group (or fixpoint
iteration) reads them, so never after the last change: it prints the module back out (passes
run before the TS strip, so it is still TypeScript), reloads the oracle and re-runs the whole
analysis with that text, without TypeScript's diagnostics. A semantic build numbers nodes in
traversal order, so the reparsed text and the rewritten AST line up node for node except where
printing changed the tree: parentheses added or dropped, and folded numbers (`-1`, `-0`, `NaN`,
`Infinity`) printed as `-` and a literal or as a name. The refresher walks both trees in step
across those differences and maps the new facts back to the rewritten AST's ids (a
parenthesized expression gets its contents' facts, and units keep the rewritten AST's spans);
any other difference drops that module's facts to `Unknown`, and opens the sites elsewhere
that called into it. After the module is compiled the program's own facts (and stats) come
back for the others.

Cost: each refresh is one whole-program analysis plus an oracle load, about 1.1× the build's
first analysis. A build costs `A × (1 + Σ_m r_m)`, where `A` is one analysis and `r_m` is the
number of pass-group iterations that changed module `m` and were followed by another
iteration (up to `max_iterations` per fixpoint group): quadratic in program size when every
module changes. Pass groups must stay few until NodeId-stable incremental analysis
(re-interpreting only the units a rewrite touched) replaces this inside
`FactRefresher::refresh`, keeping the `FactSource` interface. Refreshing once per group across
all modules, instead of per module, would need the pipeline to run groups program-wide.

`Project::reanalyse` analyses given sources the same way, and leaves those facts in place
until the next build, `facts` or `transform`. On the dev/HMR path `Project::update` marks the
facts stale; the next `transform` re-analyses the whole program (a new entry does too), so
passes never see facts about other text. Per-update incremental analysis will replace that
re-run.

Correctness is tested two ways: `/*@trust …*/` annotations in the analysis fixtures state the
expected trust of particular expressions, and a runtime checker instruments every fixture and
runs it under node, failing on any violation (the exact list is in `tests/runtime.rs`): proven
types of expressions, parameters, bindings and symbols (instances with `instanceof`,
functions by identity), in-bounds accesses, units and expressions proven not to throw or not
to write, the callee that runs at sites proven complete, loop invariants, stable object and
class shapes, and proven-unreachable code. Escape levels, points-to sets and the other loop
facts are not checked at run time.

**Analysis cost.** The acceptance target is analysis under 2× the oxc + tsrs parse time. It
is not met: `benches/analysis.rs` measures about 3.0–3.1× on 4 cores (359–380 ms against
116–124 ms for the 2040-module corpus on a noisy machine), against 7.2–7.4× for the
sequential analysis on `main` before #6 (861–905 ms) on the same corpus;
`TRIOXIDE_BENCH_STATS=1` prints a breakdown. Making the facts independent of the chunking
(below) took the ratio from 2.8–3.0× in #6's first version (on 1820 modules) to this,
mostly through more sweeps: 9 instead of 6. The corpus includes the review fixtures, on which the analysis before the review fixes
didn't converge at all, and the review fixes made each interpretation dearer (per-expression
effect frames, implicit calls, dead zones, more escapes). Interpretation runs in parallel:

- *Chunks.* Units are split, in `unit_order`, into fixed chunks of `chunk_units` (default
  128). Each round, every chunk with dirty units interprets them against a read-only
  snapshot of the whole-program tables, writing into a copy-on-write overlay and logging each
  write as an operation. Chunks run on the oracle's workers, one tsrs checker each, each
  worker sweeping one contiguous run of chunks (so files keep their checker), cut from the
  chunks' dirty units alone: no work stealing, so which checker answers a chunk, and after
  which questions, never depends on thread timing (a checker's answers can depend on what
  it was asked before).
- *Merge.* The logs are replayed onto the tables in chunk order. A chunk whose entries (and
  those its writes read) neither an earlier chunk's writes nor an earlier replay touched
  installs its overlay directly; whether it may is decided in the merge loop, after the
  replays before it, since a replay can change entries outside its chunk's own (escaping a
  parent class, following new properties). A dependency changed by the merge dirties its
  readers, except in the chunk whose overlay already held that value. Facts never depend on
  the number of threads (a test checks this), and with one chunk on one thread a round is
  the sequential analysis's round. Rounds are monotone joins with widening as above;
  nothing reads a value it did not register as a dependency.
- *Independent of chunking.* A unit in one chunk only sees other chunks' writes at the next
  merge, so without care small chunks run units on stale inputs and the joins keep what
  they saw (a function body run on `never` arguments, a method run before its instance's
  fields are written). Three rules keep that from mattering: a function (or class
  initialiser) no caller has reached yet waits while any unit its callers have reached may
  still run, and runs as it is only when nothing else can; a field an instance's class
  declares that nothing wrote yet reads as `never` until the sweeps settle, and a builtin
  call with a `never` argument is skipped until then; once they settle, the units that did
  either run again, reading the field as missing and calling the builtin (dead code keeps
  the builtin's result type, which the Rust backend lowers by). A test checks that one unit per chunk gives the default's
  facts on every program the tests compile; only TypeScript's own answers differ, since
  chunking changes the order of the questions and a checker's answers can depend on it.
- *Rest.* Parsing and indexing run per module on all threads, the oracle's checkers are set
  up meanwhile, and the summary (node facts per module, the call graph and effects, heap
  facts) runs on all threads too. Locals no other unit reads stay in their unit's output
  and never reach the shared tables; their facts are what they held in the unit's last run
  (which saw the final tables), so they are tighter than a table entry joined over every
  run would be.

- *Giving up.* The entries a merge changed that had changed in `SATURATE_AFTER` merges
  before are given up on the merged tables, in a fixed order, so this doesn't depend on
  threads either. Sites a replayed join drops into "any object" escape after the merge, as
  those a unit's own joins drop do at the end of the unit.

- *Fewer and shorter runs.* A function no caller has reached yet (and unknown code can't
  call) waits, as above: it would only run on `never` arguments, and again once a caller
  reaches it. A loop that runs again, inside another loop or in its
  unit's next run, starts from where it last settled joined with its new entry; it stops at
  a post-fixpoint above the entry either way, and usually after one pass instead of four.

Of about 360 ms, 29–45 ms is parsing and indexing (alongside the checkers' set-up),
250–325 ms the sweeps (9 rounds, 20,570 runs of 12,040 units, 16,880 tsrs queries; about
20 ms of it merging) and 45–60 ms the summary. Earlier measurements, on one thread: of the
instructions inside `analyze`, tsrs queries are 22%, interpretation 46%, parsing and
indexing 12% and the summary 11%; allocation is about 14% of everything, spread over many
small sites. Measured but not taken: widening loops one pass earlier saves 8% but loses the
bounds of counted loops; mimalloc as the global allocator (a choice for the binaries, not
this crate) takes about 11% off the analysis and nothing off the parse. Reaching 2× needs a
different shape rather than tuning: fewer sweeps (incremental analysis, or a schedule that
follows the call graph so callers' writes land before their callees run), and fewer tsrs
queries.

**Embedding constraints.** tsrs checkers take `&mut self` (one per thread, behind a mutex),
and its arenas are `'static`. Serial queries run on the program's first checker;
parallel ones (`TypeOracle::shared`) on the checker of the worker asking, or the file's own
checker, held exclusively. Every answer is converted before the checker is released, so
types from different checkers never meet and no tsrs handle outlives a query. Programs are built with tsrs's checker and
file-loader regions, so the oracle can free a replaced program (`free_program`, then the
shared file data and regions, in the order tsrs's own watcher uses). Updates are applied
lazily with `update_program`; files it reparses still leak, so the adapter rebuilds from
scratch after a configurable number of updates (default 100) or above a resident-memory
threshold (default 4 GiB, Linux). If that proves insufficient, the checker moves to a worker
process. A tsrs panic during a query is caught, answered as "unknown", and forces a rebuild;
after one on a worker, every parallel query answers "unknown" until that rebuild.

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

**Pipeline** (`trioxide_js::passes`). Passes 1–4 run as one fact-driven group, once, in the
order inline (1), checks (4), fold (2), dce (3): checks turn guards into literals that fold
then takes, and dce removes what the others left unused. Within the group node ids are
stable, so later passes read facts about the nodes earlier ones kept. The speed passes 11,
8, 12 and 5 (partial-eval, scalar, closure-hoist, inline-fn) follow in the same group: a
group of their own would refresh the facts of every module the foundation passes changed
(measured: 33 whole-program re-analyses over the pass fixtures, and the 14-module random
corpus building in 688 s instead of 46 s), while in the group they cost nothing. Each
rebuilds the scoping after it rewrites without renumbering nodes (`speed::rescope` puts the
old ids back), so the next pass still reads facts about the nodes it kept; the foundation
passes don't see the speed passes' output with facts, the syntactic cleanup does. A syntactic cleanup
group follows and runs to a fixed point without reading facts (`Pass::reads_facts`, so it
never triggers a refresh): `oxc_ecmascript` constant evaluation (plus exact int32 bitwise
folding, which it leaves alone), branches with literal tests, `null?.x`, statements after a
`return`/`throw`/`break`/`continue`, empty statements, and unused bindings with inert
initialisers. With these two groups no module is refreshed at all (refreshes happen only
before a fact-reading group that follows a change); a later fact-reading group pays for a
refresh of every module changed before it (`trioxide_js`'s `no_refreshes` test keeps the
fixture corpus at zero). An import guard brackets both groups: when the passes remove an
import's last value use, the TypeScript strip would elide the whole declaration and its module
might not run, so a side-effect `import "…"` is added in its place.

The speed passes join the fact-driven group after dce, so they read the same facts and add no
refresh: S5b's licm (9), hidden-class (10), for-of (7) and pipelines (6), then S5a's
partial-eval (11), scalar (8), closure-hoist (12), params and inline-fn (5). Pipelines run
before closure-hoist, which would otherwise lift a chain's callbacks out of its calls. Each pass
rebuilds the scoping with `speed::rescope` after rewriting, keeping node ids; a loop a pass
creates has no facts, so LICM doesn't see the loops `for…of` and pipeline lowering make (they
read `length` once themselves where the loop facts allow).

**Speed passes** (S5b; each module's doc comment has the full preconditions):
- *Pipelines (6)*: a chain of `map`/`filter` ending in `map`, `filter`, `forEach`, `reduce`,
  `some`, `every`, `find`, `findIndex` or `includes` (one call too) that initialises a binding,
  is returned, or (`forEach`) is a statement becomes one indexed loop with the callbacks
  written in. The receiver must be a plain array with intact builtins, and each call's site
  complete with only its callback as callee; callbacks are arrows or function expressions
  (no `this`/`arguments`/`new.target` of their own, no self-reference, a body that is an
  expression or ends in its only `return`). Fusing interleaves callbacks, so it needs every
  callback free of writes, throws and unknown calls, or exactly one effectful callback that is
  first (the others reading no mutable state) or last (and unable to reach the array); before a
  short-circuiting terminal (`some`, `every`, `find`, `findIndex`, `includes`) every stage must
  be free of effects, since the loop stops early. The result goes to a fresh temporary and the
  original declaration follows the loop (`const total = result;`), keeping its TDZ and
  all-or-nothing assignment when a callback throws. Hole
  semantics are kept with an `i in array` guard unless the arrays never have holes and no
  callback can make one; a `map` stage keeps holes, and a chain that would hand one to `find`,
  `findIndex` or `includes` stays. The range is fixed at the start and elements are read per
  step. Deviation: a callback after a `filter` can't use its index, and a callback after the
  first can't use its array parameter (the chain stays); calls on `Declared` receivers stay
  even with `trustTypes`, because the analysis doesn't model their callbacks.
- *`for…of` (7)*: over a plain array with intact builtins it becomes an indexed loop that
  re-reads `length` each step (once, when the loop facts show no resize), declares the head
  binding in the body (per-iteration, any pattern), and keeps labels. `[...a]` and
  `[...a, ...b]` over arrays that never have holes become `slice`/`concat`. When the body
  declares the array's name the array goes to a temporary in the head. A destructuring head
  needs elements that are never nullish. Deviation: spread calls (`f(...a)`) stay: engines
  already compile a spread call of a packed array like `apply`, and tuples can be resized.
- *LICM (9)*: hoists `LoopFacts::invariant` expressions that read a property or call
  something into the `for` head (`for (let i = 0, itemsLength = items.length; …)`) or a `const`
  before the loop. An invariant of a `for`/`while` test is evaluated by the first test anyway;
  one in the body moves only if it cannot throw (literals, number-typed bindings, arithmetic,
  unpatched `Math.*`), since a zero-trip loop must not start evaluating it. `a.length` of a
  binding declared outside the loop that nothing reassigns, holding only arrays the loop
  doesn't resize (complete only without `resizes_untracked`), is hoisted too, since the
  analysis leaves it out of `invariant` whenever the loop writes an object; not in loops that
  `await` or `yield`, where other code can resize the array. Loops with a `const` head are
  left alone, and names for a `var` head are fresh in the whole function. Deviation: pure
  arithmetic over locals stays where it is (engines hoist it themselves; naming it costs
  readability).
- *Hidden-class stabilisation (10)*: for a base class whose shape is `Proven` and whose
  instances' keys nobody observes (`instance_keys_observed`; an observer operand with unknown
  points-to, or a spread argument to one, counts as observing every class), properties instances get late or
  only on some constructor paths become class fields initialised to `undefined` (unless on the
  prototype chain, where a field would hide the inherited one), placed before the first own
  field so no initialiser's write is overwritten, and `delete o.p;` of such an
  instance becomes `o.p = undefined;`. Deviation: array element kinds aren't touched.

**Speed passes** (S5a). All four only rewrite code in their own module, and leave WASM island
functions, and code proven never to run, alone. Inlining (`"inline-fn"`) and scalar
replacement (`"scalar"`) are **off by default**: V8's optimising tiers already inline and
scalar-replace hot code, and `benchmarks/passes/` measures them neutral or slower there
(0.82–1.00×) and faster only in cold code (1.19–3.2× with `--no-opt --no-maglev`). Partial
evaluation and closure hoisting are on. With `passes.specialize`, the `"params"` pass removes
parameters every caller passes the same literal, whether or not inlining is on.
- *Inlining (5)*: calls of the module's function declarations and class methods (a method's
  receiver, a name that is never reassigned or `this` (outside derived-class constructors),
  proven not nullish, stands for `this`) when
  `site_resolution` is complete with that one callee; up to 40 AST nodes, or 400 for a
  callee's only call site (`callers_complete`). A body that is one `return` becomes an
  expression anywhere (temporaries declared at the top of the enclosing statement list);
  other bodies are inlined where the call is a statement, a declaration's initialiser, a
  `return` argument or an assignment to a name, `return`s becoming assignments and a
  labeled-block `break`. Arguments evaluate once, in order: literals and never-reassigned
  names are copied, one computed argument read first and once by the body goes into that use,
  anything else gets a local. Callees using `this` (functions), `arguments`, `new.target`,
  `super`, `eval`, `with`, labels, `var` or private names, generators, async and recursive
  functions, and callees containing a function or class are not inlined. Every call of a
  function inlined, the declaration goes (not exported, `callers_complete` usable-true). With
  `passes.specialize`, the `params` pass removes a parameter every caller (all direct calls in
  the module, no `arguments`, no defaults) passes the same literal from the signature and the
  calls; it becomes a `const` (pass 1, earlier in the group, has already folded its reads
  where specialisation allows; the cleanup drops the `const` once unread). Callees in other
  modules are not inlined: the passes see one module's AST at a time.
- *Scalar replacement (8)*: needs `escapes = None` and a stable shape for the literal's site,
  `points_to` exactly that site at every key use, literal keys present at creation, and no
  use of the binding but key reads and writes in its own function (`p.x` → `p_x`). A literal
  that holds an anonymous function or class stays (it is named after its key), and so does
  one with no facts (made by an earlier pass). `new C(…)` is replaced the same way (S10) when
  `C` is a class of the module with no heritage, decorators or instance accessors, literal
  (or no) field initialisers, and a constructor that only assigns parameters or literals to
  `this.key`; the site must be a non-escaping instance of exactly `C`, the callee proven
  pure (not in its temporal dead zone), and arguments the constructor ignores pure. The
  analysis supports it: a constructor that creates no closure and has no direct `eval` no
  longer makes its instance escape just by receiving it as `this` (passing, storing or
  returning `this` still does). Methods aren't inlined first: inline-fn stays opt-in, and
  a method call keeps the instance whole.
- *Partial evaluation (11)*: a call in module top-level code of a module function, with
  constant arguments, is interpreted (200,000 steps; numbers, strings, booleans, arrays and
  plain objects it creates, earlier module `const`s, the exactly specified parts of `Math`,
  `Number` and `String`) and replaced with a literal of at most 128 values when the call has
  no unknown calls (`expr_effects`, so builtins are the real ones) and the result is a tree
  of fresh objects with no holes. Anything it doesn't model, a throw or running out of fuel
  leaves the call.
- *Closure hoisting (12)*: a closure that captures nothing (only module-level and global
  names; no contextual `this`, `arguments`, `super` or `new.target`; no private names, which
  only parse inside their class) with `escapes = None`, that is either a `const` only ever
  called after its declaration, from its own function (it becomes a top-level
  function declaration, which pass 5 may then inline) or a callback argument of a call at
  which only it runs (`xs.map((x) => …)` on a proven array). A callback stays an arrow, in a
  `const` at the top of the module when the module has no imports or re-exports (an import
  cycle could otherwise run the function before the `const` exists), else just before the
  outermost loop around it in its function: V8 optimises a builtin's arrow callback better
  than a function declaration (2.5× in `benchmarks/passes/`).

**Specialisation** is opt-in (`passes: { "specialize": true }`). In a closed world a
parameter's facts are whatever every caller passes, which can make a function's body constant
(`y < height` becomes `y < 4`): sound, but it fights readable output and makes a `"use wasm"`
kernel's glue disagree with its body. With it off, no fact is folded about an expression that
reads a parameter, `this` or a local assigned from them, and DCE only removes unreachable code
after a `return`/`throw`/`break`/`continue`; calls are still folded where they are made.

**Islands.** The facade gives each module's passes a `trioxide_js::Keep`: exports of modules
that define island functions stay (the `ready()` glue's JS fallback calls them by name, which no
import shows), calls to island functions are not folded, and island functions are not
removed; functions forced by `"use wasm"` or `@trioxide wasm` are never removed. After an
update, modules that dropped unused exports are invalidated too, since the edit may now import
one.

**Termination** is not proven: a pure call that never returns (an infinite loop, a stack
overflow) may be removed or folded, so the output can finish where the original didn't. This
is accepted.

Rules every pass follows: one helper (`Slot`) decides where a pass may rewrite, looking through
parentheses and TypeScript-only wrappers: never a `delete` operand or a whole expression
statement, never a value fold in a callee, never an identifier under `typeof`, and a member or
`eval` replacing a callee becomes `(0, x)`; facts inside proven-unreachable code are not used;
nodes a pass creates have no facts (`NodeId::DUMMY`); expressions that call `super(…)`, `await`, `yield` or `delete` (optional chain or not; `checks` drops a `delete p?.x`'s `?.` only when `p` is proven non-nullish)
are never dropped, whatever their purity says (binding `this` and suspending are not effects
in the facts); `var` and function declarations are never removed with the branch or dead code
that contains them while referenced; literals are printed as unary minus over a literal, and as
`0 / 0`, `1 / 0`, `void 0` where `NaN`, `Infinity` or `undefined` is shadowed; strings over 32
bytes, or with lone surrogates or invisible characters, are not copied into uses; detached comments (separated by a blank line) of a removed
statement move to the next one.

---

## 5. Partitioning (JS vs WASM)

**Eligibility.** A function can become WASM only if its body is in the compilable subset:
- parameters and return types are numbers, booleans, typed arrays, `number[]`, or plain-data
  classes and objects that do not escape across the boundary (`docs/ABI.md` has the exact
  rules). In v1, `number[]` never changes length inside an island (no `push`, no
  out-of-bounds writes); a returned array must be newly created, never a parameter; and a
  returned struct must be an object literal, not a class instance
- the body uses only local control flow, arithmetic, `Math.*`, typed array and array
  indexing, loops, calls to other eligible functions, and non-escaping struct-like
  classes/objects
- no DOM or host APIs, no closures capturing JS objects, no dynamic property keys, no
  exceptions crossing the boundary, no `async`/generators, and (in v1) no strings

**Clustering.** Take connected components of eligible functions in the call graph. Every
call edge into a component from JS code is a boundary entry.

**Cost model** (per cluster; `trioxide_partition::CostModel`, `costModel` in `trioxide.json`).
Every weight is in nanoseconds, fitted to `benchmarks/partition/` (Node 22; its README has the
measurements, the fit and the confusion table) and meant to be refitted as the backend
improves. S9 refitted on main after the analysis proved loop counters int32: the fit picks the
faster side on 16 of 17 kernels (the previous weights 15), and on the whole programs it moves
`examples/image-filters`' `boxBlur` back to JS and `numeric-driver`'s `fir` into WASM, both
measured right, so the defaults are now the fitted values. The remaining misses are code the
backend still generates in `f64` (`benchmarks/README.md`):
- *benefit*: the operations the island would run per run of the program, by kind, each
  times the time WASM saves per operation (negative where the island is slower): float
  arithmetic; int32 operations (operands the backend holds as int32: every local in them has
  an int32 range fact, as the backend types locals by their binding's range); `ToInt32` of a
  float operand of a bitwise operator or `Math.imul`; `(a + b) | 0`-style wrapped int32
  arithmetic; `Math.*` calls; float `%` (an `fmod` call in WASM); element accesses with an
  int32 and with a float index; loop iterations. Loops multiply by their trip count: from
  the loop test when it compares the counter (an induction variable of the loop facts, plus
  or minus a constant) with a value whose range is known, else a default (`trips`, 256);
  typed arrays reaching a parameter from allocation sites of known length give `p.length`
  bounds and marshalling sizes, and `for…of` over array literals their length. Callees count
  at their call sites' frequency; a recursive call counts as a loop. Counts saturate at 10¹⁸.
- *cost*: boundary crossings, marshalling (a cost per array or object argument or result per
  call, plus per byte copied in, and out again when the island writes it; a `number[]` costs
  a multiple per byte, as it is converted element by element), plus a fixed instantiation
  cost per island (per module of its exports). Crossings count only the calls the facade
  redirects: direct calls of an exported, never-reassigned binding from its own module or a
  module importing it directly, weighted by the trip counts of the loops around the call
  (only the caller's own loops: a caller itself called from a hot loop counts once). The host
  calling an entry module's export, callbacks, namespace imports and re-exports run the JS
  version, so they don't count, and a function only they call is never an export.
- an island is kept if `benefit − cost > threshold` (`threshold`, 0 in `auto`;
  `wasmPreferredThreshold`, −1 ms, in `wasm-preferred`); otherwise it stays JS (and still
  gets the JS passes). The cluster's members with redirected calls become the island's
  exports; the others are compiled in as internal functions. An island that doesn't build
  (no toolchain, a cargo error) runs as JS, and its functions' verdicts say so; `explain`
  checks the toolchain and says when a build would fall back.

**Automatic islands use the forced islands' machinery** (`trioxide_rust::islands`): the
chosen functions are lowered with the forced ones, one island per module (a module's
forced and chosen functions share it), so call rewriting, `ready()` glue with the JS
fallback, and `Keep` (computed from the partition's islands, which now include the automatic
ones) are the same for both. A chosen function that fails the final lowering stays JS.
`trioxide_rust::eligibility::candidates` checks every top-level function declaration with one
parse of the program (lowering each module's candidates together, then one by one only if
they don't settle together).

**Overrides.** `"use wasm"` forces an island; it is an error if the function is ineligible,
and the diagnostic explains why. `"use js"` forbids one. Config globs do the same per file:
`overrides: [{ "files": ["src/kernels/**"], "target": "wasm" }]` (globs relative to the root,
later entries win) places a module's top-level functions as a file directive would, below the
source's own directives. Directives and overrides apply in every mode; `wasm.enabled: false`
(`--no-wasm`) turns all of it off. `mode: "js-only" | "auto" | "wasm-preferred"`: `js-only`
creates no island on its own.

Directives are implemented (S6; the cost model is S8's): a `"use wasm"` function directive, a
file directive (every top-level function declaration of the file) or a `/** @trioxide wasm */`
tag forces a function; `"use js"` (or `@trioxide js`) keeps a function, and anything calling
it, out of every island. Directives override `mode`; `wasm.enabled: false` (`--no-wasm`) turns
them off. Only top-level function declarations can be forced. The forced functions of one
module form **one island** (one glue module and instantiation per module), exporting them and
containing every function they call, from any module, as internal Rust functions; a forced
function that can't be compiled is an error listing every reason, and the module's other
forced functions still become the island. A forced function the analysis proves nothing calls
(it is unreachable, or a parameter never receives a value) stays JS with a logged reason.
`trioxide_rust::eligibility::clusters` computes the connected components of eligible
functions for the cost model.

*Default-mode reach (S8, measured with `benchmarks/partition/reach.mjs`).* The analysis now
proves typed arrays allocated anywhere in the program, `number[]` elements from literals,
`push` and `map` of proven numbers, and fresh object literals' fields at struct parameters,
so 63 of 92 functions across the benchmarks, the hand-written islands and the examples
qualify without `trustTypes` (90 with it). What still needs `trustTypes`: the arguments of
exported entry functions (the host passes them; the glue could check them at the call and
fall back to JS, which would make them provable); parameters reached through function values
held in arrays; typed arrays from builtins the heap doesn't model (`subarray`, buffer views,
`TextEncoder`); elements of nested arrays and of `Array.from` with a callback; fields of
objects from factories with default parameters or object spread.
`trioxide_rust::ExtraFacts::param_field` is the hook for field facts at entry.

**Explainability.** `trioxide explain [file[:fn]] [--entry <file>…] [--json]` plans the
program (analysis, partitioning and the JS passes, no island builds: `Project::plan`) from
`--entry`, else the module scripts of the root's `index.html`, else the target file (with a
note that its exports' arguments are then unknown), and
prints, per function, the decision (JS, WASM, forced or forbidden), why, every failed
eligibility rule with its position, the cost-model numbers (benefit, each cost term,
threshold, and the calls, operations and bytes behind them) and the JS passes that fired or
skipped inside it. `--json` prints the `Report`; the npm package's `explain()` returns the
same. This is how users come to trust the partitioner.

---

## 6. Rust backend and WASM

**RIR (Rust IR).** A small typed tree IR: functions, structs, `f64`/`i32`/`u32`/`bool`
locals, slices into linear memory, loops, and calls. It is lowered from the oxc AST plus
analysis side tables. The emitter prints Rust source (via `prettyplease`) and returns it
in memory as a `trioxide_wasm::IslandSource` (the `IslandSpec` plus `src/` files); the
builder stages it on disk and writes the manifest itself. `docs/ABI.md` specifies the exact
shape of the generated crate.

**Semantic fidelity rules.** The differential tests enforce these:
- `number` → `f64` by default; `i32` only where range analysis proves the value is an
  integer and that no operation overflows (e.g. `x | 0`, loop counters bounded by `.length`)
- JS `%` on f64 matches Rust's `%`; `Math.round`, `Math.sign`, `Math.max/min` (NaN, -0) get
  JS-exact implementations in `trioxide_rt`, checked bit for bit against golden tables
  generated by Node; transcendental functions use `libm` (the spec allows
  implementation-approximated results). Six (`tan`, `asin`, `acos`, `atan`, `expm1`,
  `log2`) match V8 exactly on the tables and on ~2M generated inputs each
  (`trioxide_rt::math::EXACT_TRANSCENDENTALS`); the others differ by at most 2 ulps on some
  inputs, so S6 doesn't lower them (an opt-in, or lowering where the last bit can't reach
  output, is future work; `docs/ABI.md`, "Numerics")
- `trioxide_rt` is `no_std`, but islands build it with its `std` feature: stable Rust only
  reaches the wasm `f64.sqrt`/`floor`/`ceil`/`trunc` instructions through `std`, and libm's
  software versions measured 15× slower. Generated code is panic-free, so `std`'s panic
  machinery is dead code
- bitwise ops apply ToInt32/ToUint32 explicitly
- array indexing: `Proven` in-bounds accesses are unchecked; anything else uses a checked
  helper that returns `NaN` for out-of-bounds numeric reads (JS's `undefined` coerced in
  arithmetic). An unprovable pattern where OOB would change semantics makes the function
  ineligible
- integer division and NaN comparisons follow JS

**How the backend works (`trioxide_rust`, S6).** Lowering *is* the subset checker: a function
is eligible exactly when it and every function it calls lower to RIR with JS-exact semantics,
so the checker can't disagree with the code generator. Each unsupported construct records a
reason with its span; reasons that only echo another failure (a value whose own lowering
failed, a failing callee's result) are dropped, so diagnostics name the cause.

- *Kinds before types.* Lowering runs in rounds over the island: the kinds of locals,
  parameters, returns and fields (boolean, number, number-or-`undefined`, array with its
  origin, immutable object, object parameter) are joined over every assignment, argument and
  return until nothing changes; a final round picks Rust types and produces RIR. Exported
  functions' parameters come from the facts at the boundary; internal functions' from their
  island call sites. Object values carry their field kinds, so `new Vec2(xs[i], ys[i])` and
  `new Vec2(a.x + b.x, …)` differ.
- *`undefined` as NaN.* A read the analysis can't prove in bounds is `number | undefined`. The
  island represents it as NaN and only allows it where JS converts it to a number anyway
  (arithmetic, ordering comparisons, bitwise operators, `Math.*`, typed array stores,
  truthiness, `===` against a value that is never `undefined`); returning it, storing it in a
  `number[]` or an object field the boundary sees, or comparing two of them makes the
  function ineligible.
- *Evaluation order.* Rust evaluates an assignment's right-hand side before its place, and
  `x op= rhs` reads `x` after `rhs`; JS does the opposite. An array store evaluates its index
  into a temporary first when the stored value may assign a variable (or both have effects),
  and a compound assignment whose right-hand side may assign the target is written out
  (`y = y + …`). Elsewhere Rust's left-to-right operand order matches JS.
- *Dead code* (statements the analysis proves unreachable) has no facts and never runs; it
  lowers to `unreachable!()`.
- *Module bindings.* An island inlines module constants and has classes from the start, while
  JS throws if code runs before their declaration. Reading a constant needs the analysis to
  prove the read can't throw; constructing a class needs nothing but declarations before the
  class in its module; and neither is allowed from a module in an import cycle (whose
  importers can run first, which the analysis doesn't model).
- *Integers.* A local, parameter or return is `i32`/`u32` only when its symbol's (or every
  returned value's) range fact proves int32/uint32 without `-0`; values computed in `f64` are
  then converted exactly. An operation runs on `i32`/`u32` only when both operands do and the
  node's range fact proves the result in range; `+`/`-` on int32s feeding ToInt32 wrap, as JS
  does. Everything else is `f64`.
- *Arrays.* Locals holding arrays are either created in the call (`&'static mut [E]`) or
  read-only (`&[E]`); writes through a variable that may hold a parameter, copies between array
  variables, returning a parameter and passing one array twice to a callee that writes to it
  are ineligible. Accesses are unchecked only where `in_bounds` is proven (a declared fact that
  is wrong would read or write outside the array), and a `number[]` element counts as defined
  only with proof too, since TypeScript's `number[]` doesn't rule out holes. Besides the facts,
  the backend proves two island-local bounds, used only for eligibility (whether a `number[]`
  write may grow the array, whether a read may be `undefined`) and never to skip a check:
  `a[i]` and `b[i]` inside `for (let i = k; i < a.length; i++)` when only the update writes
  `i`, `a` is never reassigned and `b` was created with `a.length` elements; and a constant
  index below an array's constant length. Both hold because array lengths can't change inside
  an island.
- *Objects.* Object parameters become boundary structs (one per declared type, or per
  parameter for inline types; a union of object shapes is one too). Their fields are
  the ones the island accesses, each typed by the facts of its reads (a field the island only
  writes, or that a read can't prove a number or boolean, is ineligible); this leans on the
  analysis heap being flow-insensitive, so a proven read type holds at function entry too.
  `ExtraFacts::param_field` is the hook for a direct "fields at entry" query. Classes used only
  inside the island are immutable Rust values (`Copy` structs with methods); they need a
  usable, stable class shape and a constructor that only assigns fields. Exported functions
  may return object literals, never class instances.
- *Facts the analysis doesn't provide yet* go through `trioxide_rust::ExtraFacts`, whose
  defaults answer "unknown".
- *The emitter* builds Rust text, parses it with `syn` and prints it with `prettyplease`;
  `// SAFETY:` comments, which `prettyplease` would drop, travel as numbered marker macros.
  rustc 1.99.1's LLVM rewrites `0.0 / sitofp(x)` to `copysign(0.0, x)` when the quotient feeds
  a multiplication, losing the NaN of `0 / 0` (found by the random-program differential
  tests); inlining can expose any division to it, so generated code passes every non-literal
  `f64` divisor through `core::hint::black_box`.

**Calling islands from JS.** The facade builds the partition's islands before compiling
modules, and only islands that built change any module: after TypeScript is stripped
(`trioxide_js::emit_with`), named imports of an island's exports whose every use is a direct
call move to an import of the island's glue (`\0trioxide:island/<id>`), resolving the
imported name through the defining module's exports (`export { sq as cube }`); the original
import stays, specifier-less if need be, so evaluation order is unchanged. An export whose
binding its module reassigns is never rewritten. Calls in the defining module itself go
through an aliased glue import for exported functions, in `ready()` glue (the default; see
§7) or when the glue has no fallback: top-level-await glue that imports its module as the
`fallback` would make that cycle wait on itself. Calls inside island functions' JS versions
stay JS, so the fallback never re-enters the island. Uses other than direct calls (a function
passed as a value, namespace imports, re-exports) keep the JS function. The `fallback` is the
module's absolute source path, which bundlers resolve and compile; `trioxide build` rewrites
it to the compiled module, relative to `<out>/_islands/`.

**ABI and glue** (specified in `docs/ABI.md`). Numbers and booleans pass directly. Typed
arrays, `number[]` and plain-data structs are copied into linear memory through an exported
bump allocator in `trioxide_rt`, reset after every call. Copy-back happens only when the
analysis shows the island mutates the argument. When a call passes overlapping arrays (or
one struct twice) and one is mutated, the glue calls the function's JS version instead.
Each island gets a glue ESM module that instantiates the `.wasm` and exports JS wrappers with
the original signatures. By default (`wasm.glue: "ready"`) it doesn't await instantiation:
until the island is ready the wrappers run the JS versions, so importing an island never
changes when modules evaluate relative to other tasks. Top-level-await glue
(`wasm.glue: "tla"`) runs islands from the first call but delays every importing module past
pending microtasks and I/O, so it is opt-in. A trap keeps the writes made before it (mutated
arguments are copied back in `finally`), and object arguments whose fields aren't plain
writable data properties run the JS version.

**Build driver.** Cache key = hash(island Rust source, `trioxide_rt` sources and version,
rustc version, profile, `wasm-opt` version). The source stands in for the RIR: it is a
function of it, and also covers hand-written islands. Cache misses are staged into one cargo
workspace in the cache directory and built by a single `cargo build`, which compiles them in
parallel over a shared target directory. A missing toolchain falls back to JS with one clear
warning, plus an `--require-wasm` flag that turns it into an error.
`Project::add_debug_island` (CLI: hidden `--debug-island <dir>`) adds hand-written islands
(the differential suite's `tests/islands/`).

---

## 7. Interfaces

**Rust facade (`trioxide`):**
```rust
pub struct Options { /* mode, world, trust_types, check, tsconfig, target, sourcemap, passes, wasm, dev, cache_dir, overrides, cost_model */ }
pub struct Project { /* graph + caches */ }
impl Project {
    pub fn new(root: PathBuf, options: Options) -> Result<Self>;
    pub fn build(&mut self, entries: &[PathBuf]) -> Result<BuildOutput>;      // whole program
    pub fn update(&mut self, path: &Path, source: Option<String>) -> Result<Invalidation>;
    pub fn transform(&mut self, path: &Path) -> Result<ModuleOutput>;        // per-file, Vite
    pub fn plan(&mut self, entries: &[PathBuf]) -> Result<Report>;          // build minus island builds, for explain
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
IslandSource, IslandBuilder}` (plus `trioxide_wasm::toolchain` for `check-toolchain`).

**CLI:** `trioxide build [entries…] --out-dir dist [--watch]`, `trioxide transform <file>`
(stdout; it plans the program from the file first, so its passes have the facts `build`'s
have, but it builds no islands, so calls to island functions stay JS calls), `trioxide explain [target] [--entry <file>…] [--json]` (entries default to the
target's file), `trioxide check-toolchain [--json]`. `--config`,
`--root`, `--mode`, `--world`, `--trust-types`, `--no-wasm`, `--require-wasm`,
`--emit=js|rust|wasm|report|facts` (`--emit=facts` with `transform` prints the analysis facts
of that file: per function, class and node, with spans and trust levels),
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
  that prints results. A Node runner executes four variants (types stripped only,
  trioxide JS, trioxide JS+WASM for fixtures with directives or hand-written islands, and
  `mode: "auto"` over the whole corpus and the examples, so every island the partitioner
  picks is checked) and asserts identical output. CI fails on any divergence. CI also runs
  the auto leg with every eligible cluster as an island (`TRIOXIDE_EVERY_CLUSTER=1`), which
  stresses automatic islands far beyond what the cost model picks.
- **Generated programs**: a small generator of random programs in the numeric subset
  (expressions, loops, typed arrays), fed through the differential runner, to fuzz the Rust
  backend's semantics (`tests/differential/random.test.js`: 500 programs with a fixed seed in
  CI, `RANDOM_SEED`/`RANDOM_PROGRAMS` for longer runs).
- **WASM lowering fixtures**: `tests/wasm/<rule>/` (one per lowering rule, run by the
  differential suite) and `"use wasm"` variants of every hand-written island
  (`tests/islands/<case>/wasm/`), with the generated crate snapshotted in `generated.snap`;
  `tests/wasm-errors/<rule>/`, one per eligibility rule, snapshot the diagnostics. Fixtures whose
  `main()` passes values the analysis only knows from TypeScript (subarray views, arrays with
  holes, objects kept in arrays) set `trustTypes: true` in their `trioxide.jsonc`.
- **Conformance smoke** (`tests/conformance/`, `TRIOXIDE_CONFORMANCE=1`, its own CI step):
  four vendored, dependency-free TS libraries (mitt, heap-js, neverthrow, simplex-noise, with
  their licenses) built stripped, in the default configuration, with `trustTypes` and with
  every pass on. Each output must parse, pass the library's own test suite (run unchanged
  by vitest) with the same per-test results as the stripped build, and print what the
  stripped build prints from a deterministic driver.
- **Benchmarks** (`pnpm --filter trioxide-benchmarks bench`, which writes one markdown report):
  one harness (`benchmarks/lib/harness.mjs`) runs every variant in processes of its own over
  rounds in alternating order and reports the median and the range of the speedups, under
  V8's defaults and cold (`--no-opt --no-maglev`: V8's optimising tiers already inline and
  scalar-replace hot loops, so passes 5 and 8 pay off in code that isn't hot yet). Suites:
  each JS pass on its own case (`passes/`); whole programs (the examples and realistic
  programs in `programs/`) stripped, as trioxide's JS, with every pass and in `auto` with WASM;
  the cost-model calibration kernels (`partition/`, JS-only against WASM, from which
  `fit.mjs` fits the weights); and compiler throughput (`trioxide build` wall time and peak
  memory on the conformance corpus and the analysis bench's corpus). Plain Node timing, no
  tinybench; criterion stays for the analysis bench (`cargo bench -p trioxide_analysis`).
  `benchmarks/README.md` records the numbers and the machine.
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
