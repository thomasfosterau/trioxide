# Review of PR #5 (trioxide_analysis)

Four reviewers worked in parallel, one per area: interpreter control flow, expressions and
calls, numerics and builtins, and trust/escape/`Facts` API. Every finding below was
reproduced on the PR head (`19276a7`). Each repro fixture is in
`tests/fixtures/analysis/review-*/` on this branch.

**What holds up:**
- **Overall design:** tracked locals are only those written by their own unit, tables grow
  monotonically, and dependency tracking is per entry.
- **Interval arithmetic:** `numeric.rs` passed a new 200k-case randomised property test
  (`numeric::review_props`, added on this branch) against exact JS semantics. The test also
  caught two bugs injected deliberately to check it.
- **Correct cases:** many adversarial cases are handled correctly; the per-area lists are
  below.

**What doesn't:** about 30 wrong `Proven` facts. Passes trust these, so each would be a
miscompile once they exist. Most fall into a few root causes, which are better fixed as
causes than one at a time.

The runtime harness (`tests/runtime.rs`) flags 21 of the `review-*` fixtures. The other 6 are
wrong facts it doesn't check (in-bounds, effects, shapes, convergence); see `--emit=facts`
for those.

---

## 1. Blockers: wrong `Proven` facts

### A. Control flow (`interp/stmt.rs`, `interp/narrow.rs`)
| # | Bug | Fixture |
|---|-----|---------|
| A1 | `break`/`continue` out of `try…finally`: the break env is saved before `finally` runs, so the finaliser's writes are lost (stmt.rs ~625–685, Break/Continue ~135) | `review-finally-break` |
| A2 | A throw from `catch` reaches `finally` without the catch block's writes (`thrown` only accounts for `try`) | `review-catch-finally` |
| A3 | Function declarations in `switch` are hoisted per case, not for the whole case block, so an earlier case's call reads `Never` and that path is dropped | `review-switch-hoist` |
| A4 | Each `switch` case test is evaluated from the entry env, losing earlier tests' side effects (the default/no-match exit has the same problem) | `review-switch-test` |
| A5 | Narrowing is applied to the post-operand env: `x === ((x = "a"), 1)` narrows the new `x` with a fact about the old one | `review-compare-assign` |

### B. Implicit calls and object model (`interp/expr.rs`, `interp/call.rs`)
| # | Bug | Fixture |
|---|-----|---------|
| B1 | Static getters are stored as data properties: `S.label` reads as `function@proven` (expr.rs:669–684, call.rs:367) | `review-expr-a` |
| B2 | Reading a setter-only accessor returns the setter function, not `undefined` (call.rs:369–373) | `review-expr-c` |
| B3 | `"01"` / `"+1"` are treated as array indices (`parse::<u32>`, expr.rs:66) | `review-expr-c` |
| B4 | A tagged template's strings array has no elements: `s[0]` is `undefined@proven` (expr.rs:139–143) | `review-expr-c` |
| B5 | A constructor that returns an object is ignored, so writes are attributed to the wrong allocation (call.rs:526–545) | `review-expr-c` |
| B6 | Unary `+` drops `.at_most(trust)`, promoting a `Declared` range to `Proven` (expr.rs:805–808) | `review-expr-c` |
| B7 | `forEach`/`map`/… ignore `thisArg`: `this` is `undefined@proven` (call.rs:928) | `review-thisarg` |
| B8 | `Math.fround` keeps its argument's range: `fround(0.1)` is `0.1@proven`, and `fround(1e300)` is not Infinity (call.rs:1061) | `review-num` |

### C. Builtin integrity (prototype patching)
The integrity check only notices patching that names the namespace on Array and TypedArray
receivers. Whenever integrity can't be guaranteed, the models must be off.
| # | Bug | Fixture |
|---|-----|---------|
| C1 | `String.prototype` / `Number.prototype` patching is ignored (`receiver_kind`, call.rs:713) | `review-strproto`, `review-numproto` |
| C2 | Typed array prototypes aren't guarded; typed array constructors are missing from `NAMESPACES` | `review-taproto` |
| C3 | Prototypes reached without naming the namespace, via `Object.getPrototypeOf`, `Reflect.getPrototypeOf`, `.constructor.prototype` or `__proto__`, bypass the check, because a member call counts as a "safe use" (mod.rs ~358, ~425) | `review-getproto` |
| C4 | `Object.prototype` pollution is ignored: `inherited()` / `super_prop` return `undefined@proven` for unlisted names (expr.rs:1750–1755, 1824–1832) | `review-expr-c` |
| C5 | Holes ignore inherited indices (`Array.prototype[1] = …`, or through `Object.prototype`), and `iterate` assumes `Array.prototype[Symbol.iterator]` is intact | `review-protoindex`, `review-expr-d` |
| C6 | `Function('return this')()` (and probably indirect `eval`) reaches the global object without being caught | `review-fnglobal` |

### D. Array length and in-bounds facts
The relation between `i`, `n = a.length` and `a` survives things that change the length:
| # | Bug | Fixture |
|---|-----|---------|
| D1 | `const n = a.length; a.pop(); for (i < n) a[i]` keeps `Env::lengths`, so `a[i]` is `number@proven` (`after_call` / `forget_bounds_of` only filter `bounds`) | `review-length-pop` |
| D2 | A `.length = k` write inside the loop, directly or through an alias (`o.a.length = 0`), doesn't drop bounds (expr.rs:1906–1911). The type stays sound, but `in-bounds@proven` is wrong | `review-bounds`, `review-expr-b` |
| D3 | Getters, setters and coercions (`valueOf`) never call `after_call`, so mutations they make are invisible to bounds (call.rs:385–398, expr.rs:408) | `review-expr-b` |
| D4 | A typed array detached through its buffer (`buffer.transfer()`, `structuredClone(…, {transfer})`) keeps its length and element type; reading `.buffer` doesn't escape the typed array | `review-bounds`, `review-expr-d` |
| D5 | Typed array length can exceed 2^32−1 (Node 22 allows `new Uint8Array(2**32+5)`), but `MAX_ARRAY_LENGTH` is applied to them (numeric.rs:16; expr.rs:1500/1597/1622; call.rs:893/1034). Use 2^53−1 for typed arrays. This also matters for PLAN §6's i32 loop counters | repro below |
| D6 (major) | In-bounds is reported on holey arrays. An unchecked index into a hole isn't safe to lower; require non-holey | `review-holes` |

D5 repro (allocates 4 GB, so it isn't committed as a fixture):
```ts
function len(a: Uint8Array): number { a[a.length - 1] = 7; return a.length + a.indexOf(7); }
export function main(): void { console.log(len(new Uint8Array(2 ** 32 + 5))); }
```

### E. Trust, openness and effects
| # | Bug | Fixture |
|---|-----|---------|
| E1 | `Pts::join(Some(sites), None)` gives `None` but never escapes the dropped sites. So `const f = c ? known : unknownValue; f(1)` leaves `known`'s parameter `never@proven` (value.rs) | `review-partial` |
| E2 | In app mode, entry-module exports are treated as never called (`main(flag?)` gets `flag: never@proven`), but hosts, HTML module scripts and test runners call them (interp/mod.rs `write_symbol` ~1240). Entry exports must be open; fix PLAN D2 too | `review-entry` |
| E3 | Effects ignore TDZ: calling a function that reads a not-yet-initialised `let`/`const`/`class`, including across circular imports, is marked pure. DCE would remove a throw | `review-tdz`, `review-circular` |
| E4 (major) | Class and object shapes miss keys added in the creating unit: `new P(); p.z = 3` leaves `P` "stable" (mod.rs `store_prop` ~1300) | `review-shapes` |
| E5 (suspected) | `import dep = require("./dep")` on a graph module doesn't escape `dep`'s exports | — |

## 2. Majors: whole-program fact loss (safe, but a cliff)
Non-convergence drops every fact in the program and reports only an `Advice` diagnostic.
Two ordinary patterns trigger it:
- **Unions aren't widened (F1):** a growing range inside a union (`let s: number | null`
  incremented globally) is never widened. `join_into` (mod.rs ~771) and `Env::widen` only
  widen a top-level `Number`. Widen recursively. Fixture: `review-union-widen`.
- **`delete` oscillation (F2):** one `delete` of an object literal's only static key makes
  `present` oscillate (expr.rs ~577). Fixture: `review-shapes2`.

Suggestion: on non-convergence, fall back per unit or per SCC to `Unknown` rather than
dropping the whole program, and make it a warning.

## 3. Minors
- `typeof u === "object"` on an Unknown value narrows its type to `Never`, masked by trust.
  This is fragile if anything tests `ty == Never` for reachability.
- **Writes that throw:** writing a getter-only accessor stores a data property; in strict mode
  it throws.
- **Double evaluation:** compound assignment evaluates the member object twice (imprecise).
- **`fill`:** keeps the array holey, and stores `start`/`end` as elements.
- **`Allocation.escape`:** always `Fact::proven`, even when the decision rested on `Declared`
  edges.
- **CLI:** still warns that `--world` / `--trust-types` have no effect (cli/src/lib.rs:404).
  `transform <relative path>` resolves the path twice.
- **Dead code:** `summary.rs:319` `|| info.has_heritage` is never reached.
- **Doc mismatch:** `canonical_cmp`'s doc says it agrees with `==`, but `NumberType` uses
  `total_cmp`.
- **Duplication:** `analysis_inputs` and the JSX defaults are duplicated between `project.rs`
  and `tests/common`.
- **TS-invalid input:** a base-class field shadowing a subclass getter (TS2610) is analysed
  as the getter.

## 4. Gaps in the runtime harness
Contrary to PLAN §3's wording, `tests/runtime.rs` checks only `Facts::ty`, and only on some
node kinds. It never checks:
- **Fact kinds:** `in_bounds`, effects, escape, `stable_shape`, class shapes, call edges,
  `symbol_ty`.
- **Nodes:** parameters (`BindingIdentifier`), template literals, sequences, function and
  class expressions, `yield`, `super`, anything inside `?.` chains, and assignment-target or
  callee positions.

The predicates are also weak: `Instance` checks only "is an object" (use `instanceof`), and
`Function` checks only `typeof`. And only the executed path is checked: no fixture's `main`
takes parameters, which is how E2 slipped through.

Add runtime checks for:
- **`in_bounds`:** `index < arr.length`, and the array has no hole at `index`.
- **Effects:** wrap calls marked side-effect-free, and assert they make no global writes and
  don't throw.
- **Shapes:** at the end of the run, check the key order of stable classes and allocation
  sites.

Also skip cleanly when `pnpm install` hasn't been run. The `decorators` fixture currently
hard-fails.

## 5. `Facts` API gaps the next sessions need
Passes (S4/S5) and the Rust backend (S6) will need:
1. **Call-site completeness:** `site_resolution(site) -> { callees, complete: Fact<bool> }`.
   `callees` doesn't say whether unknown code may also be called (in E1 there was no edge at
   all).
2. **Per-expression effects:** `expr_effects(node)` / `is_pure(node)`, covering getters,
   ToPrimitive and property reads. This is for DCE and LICM.
3. **Loop facts:** per loop, which locations are written; whether an array's length is
   mutated in the loop; loop-invariant expressions; induction variables. This is for passes
   6, 7 and 9 and for the i32 lowering.
4. **Points-to per expression:** node → allocation sites, so scalar replacement can pair
   `p.x` with an `escape = None` site.
5. **Reachability fact:** `never` currently means both "unreachable" and "no caller seen".
6. **`callers(f)` index:** for single-call-site inlining.
7. **Refreshing facts after rewrites:** this is the biggest gap. Facts are keyed by
   `NodeId`, which every rewrite renumbers. `analyze()` takes source strings, and the tsrs
   oracle's spans match only the original source. `FactSource::refresh` is a no-op.
   Recommended for v1: re-analyse from regenerated source between pass groups. Passes run
   before the TS strip, so the output is still TS and tsrs can reload it. Keep the number of
   pass groups small, and give each pass a declared list of the facts it reads. Incremental,
   NodeId-stable facts can come later, together with the dev/HMR incremental work.
8. **Trust on escape and shape:** currently uniform; passes can't tell a `Declared`-dependent
   shape from a `Proven` one.

## 6. Performance
Analysis takes 5.3× the parse time against a target of 2×. That's not urgent. By instruction
count it's about 1.2× the parse; the gap is single-threaded interpretation against tsrs's
multi-core parse.

Defer the parallel (Jacobi) interpreter. Cheaper first steps:
- **Batch tsrs queries:** they are pure per node, so they can be batched or pre-warmed in
  parallel.
- **Incremental analysis for dev/HMR:** `Project::analyse` re-runs the whole program on every
  update and `clear()`s outputs.

Both the parallel interpreter and incremental analysis reshape the same tables, so settle §5
item 7 first.

## Handled correctly (no action)
- **Control flow and dataflow:**
  - `return` from `try`
  - worklist dependency tracking
  - closures, `forEach` callbacks and module-level functions that pop inside bounded loops
  - `pop`, `splice` and `shift` through aliases
  - `i` modified in the body, `<=` bounds, fractional or negative starts, `subarray`
- **Builtins:**
  - `Math.max()` is −Infinity
  - −0 through `abs`, `sign`, `floor`, `trunc` and `sqrt`
  - `parseInt`, `charCodeAt` out of range, `JSON.parse`, `sort`
  - sparse `map`/`slice`
  - typed array element ranges
- **Classes:**
  - a constructor calling an overridden method
  - `Object.create(C.prototype)`
  - cyclic `extends`
- **Escape to unseen code:** `window.foo = f`, `Object.assign(globalThis, …)`,
  `setTimeout`, dynamic `import()` and `import x = require` all escape correctly.
