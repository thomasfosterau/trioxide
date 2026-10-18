# S2 handoff: `trioxide_types` and `trioxide_analysis`

This is for the session that takes over S2. Read `docs/PLAN.md` §3 (the spec, with every
deviation recorded) and `docs/sessions/common.md` first. Paths below are relative to
`crates/trioxide_analysis/src` unless they say otherwise.

## Architecture in brief

**Pipeline** (`lib.rs` `analyze`): index the parsed modules (`world.rs` `World`), link
imports and exports (`link.rs`), run the interpreter (`interp/mod.rs` `run`), summarise
the tables into `Facts` (`summary.rs` `facts`), then record constants (`constants.rs`
`record`). The facade calls this from `Project::analyse` (`crates/trioxide/src/project.rs`).

**Types** (`crates/trioxide_types`): `ty.rs` is the lattice (`Type`, `NumberType` with range,
`int`, `non_nan`). `oracle.rs` is `TypeOracle`; the tsrs adapter (`tsrs_adapter/mod.rs`,
`convert.rs` for tsrs → `Type`, `spanmap.rs` for span mapping, `fs.rs` for the VFS) answers
"declared type at this span". Only this crate may depend on tsrs (feature `tsrs`).

**Units and worklist.** A unit is a module body, a function, arrow, static block or class
initialiser body, in `World::unit_order` (source order). `run` makes rounds over a
dirty-unit worklist. `Deps` records what each unit read (`read`), and every table write
that changes a value calls `touch`, which dirties the `readers` of that `Dep`. `Dep` is
`Symbol`, `Params`, `This`, `Return`, `Heap`, `Super` or `Default`. In the first round a
function no caller has reached yet waits (`Cx::uncalled`); warm-started loops resume from
`entry.join(settled)` via `loop_heads`.

**Tables** (`State`): `symbols`, `params`, `this`, `returns`, `supers`, `defaults`, `heap`,
`open_instances`, `may_be_uninit`, `spread_called`. A value is `value.rs` `Val`: `ty`,
`trust`, `pts` (points-to: `Sites` up to `MAX_SITES` or `Any`) and `poisoned`. When
`Pts::join` drops sites to `Any` it records them (`take_lost`), and they are escaped.

**Heap.** One `Obj` per allocation site (`Cx::alloc`, `Cx::mark`), with:
- `kind` (`ObjKind`), `props`, `order`, `present`, `elem`, `length`, `sized`, `holey`;
- `dynamic` (computed keys), `deleted`, `added`, `accessors`;
- `escape` (`None`/`Known`/`Unknown`), `shares_buffer`, `poisoned`;
- `frozen` (#19: `Object.freeze` marks it), `given_up` (sticky, see below).

Escape is propagated by `Cx::escape`/`escape_value`. An `Unknown` escape means code trioxide
doesn't see may hold the object, so its fields are unknown from then on.

**Trust.** `Proven` comes only from the interpreter. `Declared` comes from tsrs via
`Cx::declared_type` (cached in `ts_cache`) and the `or_declared` helpers, used where the
interpreter has nothing (host-supplied parameters, opaque calls). `Unknown` otherwise.
`trustTypes` makes `Declared` count as usable (`Facts::is_usable`).

**Convergence and give-up** (constants at the top of `interp/mod.rs`):
- `WIDEN_AFTER = 3`: a table entry that changes more often than this is widened
  (`numeric::widen` sends a moving bound to ±∞; types join).
- `SATURATE_AFTER = 20`: the entry gives up (`Cx::give_up`, `Cx::saturate`); for heap
  entries `Obj::give_up` makes everything unknown, and it is re-applied after every `mark`,
  so it is sticky.
- `MAX_ROUNDS = 40`: the backstop. Hitting it drops all facts; `tests/convergence.rs`
  checks that nothing in the corpus does.
- Loops: `run_loop` (`interp/stmt.rs`) iterates the body, widening after
  `LOOP_WIDEN_AFTER = 2` and stopping at `MAX_LOOP_ITERATIONS = 12`.

**TDZ rules:**
- In its own unit, a lexical read before its declaration throws (`declared_at`).
- `Cx::note_call` marks lexicals not yet declared as `may_be_uninit`, since a call may read
  them.
- Reading an import that is in an import cycle may throw (`Cx::read_import`,
  `import_cycles`).
- From another unit, a module-level lexical of a cyclic module may be uninitialised
  (`Cx::note_tdz_read`, the `cyclic ||` rule from #10). It is coarse; see the gaps.

**Builtin integrity** (`patched_builtins`): if the program writes a builtin or its
prototype, or reaches prototypes reflectively (`__proto__`, `getPrototypeOf`,
`.constructor`, `defineProperty` on them), the affected builtin models are off.
`open_modules` and `unit_open` decide which code may be called by unseen code.
`builtins.rs` holds the models and their effects.

**Summary** (`summary.rs`): `facts` turns the tables into `Facts`:
- `node_types` via `materialize`/`refine` (the heap refines object and array types);
- `usage_facts`/`module_usage`;
- `heap_facts`: shapes, lengths, holes, escape;
- `functions`, `callgraph::build` and `callgraph::effects` (`callgraph.rs`);
- `induction_variables` and `invariant_expressions` for loop facts.

`constants.rs` adds `const` literals, sound enums and unused exports.

**Edits** (`facts.rs`): `FactRefresher` re-runs analysis for passes. `remap_module` moves
facts to new `NodeId`s after a pass rewrites a module, including the `resized` remap from
#9. `forget_module` drops a module's facts.

## Invariants

- **Tables are monotone.** Writes join, and an entry never goes down a level. If a change
  breaks this, the round loop oscillates and the entry gives up.
- **Give-up is sticky.** A given-up entry never recovers, in this run or after a `mark`.
- **A missing fact means `Unknown`.** Every `Facts` query returns `Unknown` for a node it
  has nothing about (dead code, a new node from a pass). Never default to anything stronger.
- **`Proven` means the runtime harness checks it.** If you add a `Proven` fact of a new kind,
  extend `tests/runtime.rs` to check it, or say in its doc comment that it isn't checked.
- **Unsound is worse than imprecise.** Every miscompile so far (#16, #18, #19) was the
  interpreter treating something it didn't model as pure or as `Never`. When unsure, return
  an unknown value and an unknown effect.

## How to test

- **Runtime harness** (`tests/runtime.rs`). It instruments every analysis fixture with a
  `main()` and runs it in Node. Any `Proven` fact contradicted at runtime fails the test.
  - The module comment lists exactly what it checks: types, bounds, unit and expression
    effects, complete call sites, loop invariants, shapes and reachability.
  - It also lists what it doesn't check: escape levels, points-to sets, `new` sites,
    `resized`, writes by builtins and through `this` outside the unit.
  - `FIXTURE=name cargo test -p trioxide_analysis --test runtime` runs one fixture.
  - Every soundness fix gets a fixture in `tests/fixtures/analysis/<name>/` that fails here
    without the fix.
- **`tests/convergence.rs`:** no corpus program hits `MAX_ROUNDS`.
- **Other tests:** `tests/facts.rs` (query API), `tests/usage.rs`, `tests/constants.rs`,
  `tests/trust.rs` (trust levels with and without `trustTypes`), and
  `crates/trioxide/tests/project.rs` (refresh and remap through the facade).
- **#6 only:** `facts_do_not_depend_on_threads` and
  `declaration_facts_do_not_depend_on_chunking` in `tests/facts.rs`, plus tiny-chunk runs of
  the runtime harness.
- **Snapshots:** `cargo insta review` for `tests/fixtures/analysis/*/output.snap`. A
  precision change also moves S4/S5/S6 snapshots; list those under "Cross-stream changes".
- **The differential suite** (`pnpm -r test`) catches what the facts break downstream.
- **Benchmark:** `cargo bench -p trioxide_analysis` (`benches/analysis.rs`). It times
  "parse: oxc + tsrs" against "analyze (with tsrs queries)" on 1820 modules. The bar is
  analysis under 2× parse. `target/criterion` is large; delete it when the disk fills.

## State of #6 (parallel analysis): parked, draft

**Design:**
- Units are swept in chunks of 128 (`interp/sweep.rs`: `run`, `sweep_chunk`, `run_units`,
  `sweep_shared`).
- Each chunk reads the round's tables through a copy-on-write overlay (`interp/tables.rs`
  `Tables`) and logs its writes as `Op`s.
- At the merge, ops are replayed in chunk order (`apply`). `Op::GiveUp` is applied in sorted
  order. `Op::Mark` closures must be idempotent `Fn + Send` (`move`) closures, and `Mark`
  re-applies `Obj::give_up`.
- `Settled` makes `Env` sendable.
- Workers are tsrs checkers with work stealing (`threads.rs`), and parse and summary are
  parallel too.
- Facts depend on `chunk_units` but not on the thread count.

**Where it stands:** the branch `claude/s2-parallel-analysis` (`1e059cb`) is rebased onto
`c66e37d` (#14) and green. It has everything on `main` up to then: #10's `constants.rs` and
cycle TDZ, #12's `sized` and sticky give-up, #14's deferral, warm loops and escaped
parents.

**Still to port** (none of these are on the branch):
- From #18:
  - the named function expression self-binding in `interpret` (through tables/ops there);
  - `call_with` for tagged templates;
  - the computed-key array write (`store_elem`, `dynamic`/`holey`/`length = None`), as a
    `move` closure.
- From #19:
  - `delete`/`non_configurable`;
  - `Obj::frozen` (add it to the flags tuple in `tables.rs`'s `Mark` comparison, and make
    the freeze mark a `move` closure);
  - mutating builtins throwing on a frozen (or unknown) receiver (`builtin_mutates`), and
    `Object.freeze` throwing on typed arrays, namespaces and untracked objects.
- From #15 and #16, once they merge: their analysis changes (#16's `observe.rs`, the
  `delete` operand fix, new `Facts` tables in `remap_module`/`forget_module`).

**Known blocker** (from the #6 review): in `sweep.rs` the merge decides which chunks take
the fast path before the ops are replayed, so a chunk can skip work that a replayed op made
necessary. The review is adding a fixture for it; fix that before anything else on #6.

**Numbers:** about 2.8–3.0× parse (291–306 ms analysis against 102–105 ms parse). That
misses the 2× target. About 270 ms of a sequential run is outside the sweeps (single-threaded
parse, index and summary), so the next win there is parallelising the summary further.

**Risks:**
- It is a lot of concurrent code for a reviewer to hold.
- Facts depend on chunking, so a chunk-size change moves snapshots.
- A non-idempotent `Mark` closure gives thread-dependent facts that only the determinism
  tests catch.
- Every sequential change has to be ported twice, so it rots quickly. Decide whether to land
  it before taking on more interpreter work.

## Known gaps and requests, most important first

1. **Entry-function argument facts** (S8, S7). Arguments the host passes to exported entry
   functions are only `Declared`, which is the biggest default-mode gap (63 of 92 benchmark
   functions qualify without `trustTypes`, 90 with it; PLAN §5 "Default-mode reach").
   - **S8's proposal** (#15): the glue checks argument types at the call and falls back to
     JS on a mismatch, so the island may assume them.
   - **Conditions from the owner's review of #15, all required:**
     - **Scope:** facts that depend on the check may only be used by the island lowering,
       never by S4/S5 passes or the JS version, because the JS version also runs before
       `ready()` and on any mismatch. This needs a separate fact channel, e.g.
       `trioxide_rust::ExtraFacts::param_field`, not `Proven` in `Facts`.
     - **Numbers:** check `typeof x === "number"` and assume full f64 (no int or range
       facts).
     - **Typed arrays:** check the kind through the intrinsic
       `%TypedArray%.prototype[Symbol.toStringTag]` getter via `Reflect.apply`, not
       `instanceof`. Reject shared, resizable and detached buffers.
     - **`number[]`:** `Array.isArray`, not a Proxy, dense, own data properties, every
       element a number.
     - **Structs:** own data properties only, fields checked recursively, prototype
       `Object.prototype` or `null`.
     - **Aliasing:** assume arguments may alias each other.
     - **Cost:** the checks run per call and per element, so the cost model must charge
       for them.
   - S2's part is to expose, per entry parameter, the declared shape that the glue should
     check (S7 writes the glue, S8 the cost).
2. **Parameters through function values in arrays.** `const fs = [a, b]; fs[i](x)` leaves
   `a`'s and `b`'s parameters unknown. Array element points-to for functions doesn't reach
   the call-site resolution.
3. **`subarray`, buffer views, `TextEncoder`.** The heap doesn't model typed arrays from
   these builtins (`subarray` must share the buffer: `shares_buffer`), so their element kind
   and length are lost.
4. **Nested arrays and `Array.from` callbacks.** Elements of `number[][]` and of
   `Array.from({ length }, f)` aren't proven. `elem` is one level, and the `Array.from`
   callback's return doesn't flow into the result.
5. **Fields through object spread or factories with default parameters.** `{ ...a, x }` and
   `make(x = 0) { return { x }; }` lose field facts (spread copies are treated as opaque,
   and the default-parameter join widens).
6. **Loop counters widen to `[0, ∞)`.** `for (let i = 0; i < n; i++)` gets `i: [0, ∞)`
   because `numeric::widen` sends a moving bound straight to infinity. The body's narrowed
   `i` is bounded, but the binding isn't, so S6 lowers counters as f64 (S8 found every
   image-filter access doing a float-to-usize round trip). The fix is widening with
   thresholds (`MAX_ARRAY_LENGTH`, int32 max, the loop test's bound) or a narrowing pass
   after the loop fixpoint. Widening one iteration earlier was measured (−8% time) and loses
   bounds, so don't.
7. **Per-property write facts for LICM** (S5b, #16). `this.width` in a loop that writes any
   object isn't invariant because loop facts only know "writes some object". Per-site,
   per-property write sets (`resized`-style) would let LICM hoist it.

**Smaller, imprecise or fragile:**
- A class heritage read inside a function gives `unknown` (`ref Base: unknown`).
- `made?.get` resolves to `unknown` (optional member callees lose the callee).
- #10's cyclic-module TDZ rule is coarse: any cross-unit read of a cyclic module's lexical
  may throw (`review-circular`'s `readA`). A real initialisation-order model would fix it.
- About 1200 self-dirtying re-runs come from `Escape` writes to a unit's own reads. This is
  wasted time, not wrong facts.
- Only `Object.freeze` sets `frozen`, and a mutating builtin counts as throwing whenever its
  receiver may be frozen, poisoned or reachable by unknown code. `Object.seal`, `preventExtensions` and
  `defineProperty` go through reflective poisoning, which is sound but coarse.
- #16 also edits `delete` handling in `interp/expr.rs` (looking through `?.`, parentheses
  and TS wrappers). It will conflict with #19's `delete`; keep both: #16's operand
  unwrapping, then #19's throw rule on the unwrapped member.
- tsrs queries are 22% of analysis instructions. Fewer `declared_type` calls (only where the
  interpreter has nothing) is the cheapest speed-up left.
- mimalloc cut analysis time 11% in a measurement but wasn't taken (it's a pin and a
  workspace change).

## Performance

Benchmark: `cargo bench -p trioxide_analysis`, 1820 modules.

| build | analysis | parse | ratio |
|---|---:|---:|---:|
| `main` before #14 | 856–889 ms | ~104 ms | ~8.2× |
| `main` at `c66e37d` (sequential) | 773–796 ms | ~104 ms | ~7.5× |
| #6 at `1e059cb` (parallel) | 291–306 ms | 102–105 ms | 2.8–3.0× |

Of the sequential time, the sweeps take about 470 ms. The rest, about 270 ms, is
single-threaded parse, index and summary.
