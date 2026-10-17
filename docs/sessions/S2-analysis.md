# S2: Types (tsrs adapter) and analysis

Start from `main` after S1 has merged. Read `docs/sessions/common.md`, `docs/PLAN.md`
(especially §2 pipeline steps 2–4, §3, D1–D3, and the embedding constraints), and
`CLAUDE.md`. You are building milestone M2. S4 (JS passes) and S6 (Rust backend) are
blocked on your fact tables, so get the `Facts` API right early and land it in reviewable
increments: a first PR with the adapter, a second with the analyses.

## You own
`crates/trioxide_types`, `crates/trioxide_analysis`, and the analysis-related fixtures
under `tests/fixtures/analysis/`.

## Deliverables
1. **`trioxide_types`: the tsrs adapter.** This is the only crate allowed to name tsrs
   types.
   - **Program setup:** in-memory `tsrs_vfs` fed from `trioxide_project` sources,
     `tsconfig.json` parsing via `tsrs_tsoptions`, bundled `lib.d.ts`, and incremental
     updates via `update_program`.
   - **Span map:** match oxc nodes to tsrs nodes by UTF-8 byte span, using
     `get_start_of_node` / `skip_trivia` to discard TS-style leading trivia. Handle
     parenthesised expressions, JSDoc reparsing, and nodes that exist in only one AST.
     Unmatched nodes yield `Unknown`. Test this thoroughly, including a property-style test
     that walks every expression in the fixture corpus and reports the match rate, which
     must be ≥99% of identifiers and member and call expressions.
   - **Queries:** wrap get_type_at_location, symbol at location, resolved signature, type
     of a symbol, flags, and `type_to_string` (debugging only). Convert the results into
     trioxide types (PLAN §3). Recognise arrays, tuples, typed arrays, and classes by
     comparing against tsrs global types and symbols.
   - **Optional checking:** `check: true` maps tsrs diagnostics into trioxide `Diagnostic`s.
   - **Concurrency and memory:** checkers are `&mut` and per-thread; types must never mix
     across checkers. Implement the dev-mode rebuild policy (after N updates or above a
     memory threshold), and measure whether the `plain-ptrs` feature is worth enabling for
     embedding. Record the numbers in the PR.
2. **`trioxide_analysis`.**
   - **Linker:** imports to exports across modules, giving global `SymbolRef`s.
   - **Call graph:** includes calls to methods of known classes and SCC computation.
   - **Builtin models:** a data table of types and effects for `Math`, `Number`,
     `Array.prototype`, typed arrays, `String.prototype`, `JSON`, `Object.*`, and the
     common globals.
   - **Effect summaries** per function: reads, writes, allocates, throws, calls unknown.
     Bottom-up over the SCCs.
   - **Soundness refinement:** turn tsrs types into `Proven` / `Declared` / `Unknown` per
     PLAN §3 and D1, aware of world mode (D2: app vs library).
   - **Numeric range and integer analysis:** intervals, `int`, `nonNaN`, `nonNegZero`,
     with loop counter bounds from `.length`.
   - **Escape analysis** for object literals, tuples and class instances.
   - **Hidden-class shape facts.**
   - **`Facts` query API** for passes, e.g. `facts.ty(expr)`, `facts.effects(fn)`,
     `facts.range(expr)`, `facts.escapes(alloc)`, each returning the trust level. Wire it
     into the facade so passes get `&Facts`.
3. **Debug output.** `trioxide --emit=facts <file>` prints facts per node with spans; the
   fixtures snapshot this output. Aim for about 40 analysis fixtures, including adversarial
   ones: `any` laundering, casts, mutation through aliases, `arguments`, `eval`, and
   open-world exports.

## Acceptance
- All analysis fixtures snapshot cleanly.
- The span match rate is ≥99% on the corpus.
- Adversarial fixtures never produce a `Proven` fact that is wrong. Add a test that asserts
  each one's expected trust levels.
- Analysis of the fixture corpus stays under 2× the time of parsing it with both oxc and
  tsrs. Include a criterion bench.

## Out of scope
Rewriting code (S4/S5), partition decisions (S8).
