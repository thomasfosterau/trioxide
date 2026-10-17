# S7: WASM runtime, ABI, glue and build driver

Start from `main` after S1 has merged. Read `docs/sessions/common.md`, `docs/PLAN.md`
(especially D4–D7, §6 "Semantic fidelity rules", "ABI and glue" and "Build driver"), and
`CLAUDE.md`. You are building the back half of M5, everything after Rust source exists.
S6 (the TS→Rust lowering) hasn't started, so you'll drive your work with **hand-written**
island crates that look like what S6 will generate. The contract you define, `IslandSpec`
plus the Rust shape of an island, is what S6 will target. Make it precise and documented.

## You own
`crates/trioxide_rt`, `crates/trioxide_wasm`, `tests/islands/` (hand-written islands) and
the JS+WASM leg of the differential runner.

## Deliverables
1. **`trioxide_rt`**, the runtime crate the generated Rust depends on. It is `no_std` and
   targets `wasm32-unknown-unknown`.
   - **JS-exact numerics:** `Math.round`, `sign`, `max`/`min` (NaN, -0), `trunc`, `fround`,
     `clz32`, `imul`, `hypot`; ToInt32/ToUint32 and bitwise ops; JS `%`; NaN comparison
     semantics; integer division rules. Transcendentals via `libm`, documenting where
     results may differ in the last bit.
   - **Array helpers:** a checked index that returns NaN when out of bounds, unchecked
     variants, and growable `number[]` semantics if the subset needs them. Agree the
     subset boundary in PLAN §5 and keep it narrow.
   - **ABI exports:** a bump allocator (`__trioxide_alloc`, `__trioxide_reset`) and the
     memory layout for passing typed arrays and `number[]`.
   - **Tests:** host-side unit tests compare every helper against values produced by
     Node's JS engine. Generate them with a script and check them in as golden tables,
     with edge cases: NaN, ±0, ±Infinity, 2^31 boundaries, subnormals.
2. **ABI spec**, written up as `docs/ABI.md` and modelled in `trioxide_wasm::IslandSpec`:
   - which parameter and return types are allowed
   - how each is passed (direct, or pointer+length into linear memory)
   - copy-in/copy-out rules, with copy-back only for arrays the island mutates
   - struct layout for plain-data classes
   - how traps are reported, and re-entrancy rules (no JS callbacks in v1)
   - versioning of the ABI
3. **`trioxide_wasm`, the island builder:**
   - Takes an `IslandSpec` + a Rust crate directory (`Cargo.toml`, `src/lib.rs`).
   - Runs `cargo build --release --target wasm32-unknown-unknown` with tuned profile
     settings (opt-level 3, lto, codegen-units=1, panic=abort).
   - Runs `wasm-opt -O3` if it's on PATH.
   - Content-hash cache. The key is the Rust source, `trioxide_rt` version, rustc
     version, and profile; the cache lives in a configurable `cache_dir`.
   - Builds islands in parallel. Generated crates share one target dir so `trioxide_rt`
     builds only once.
   - Toolchain detection, exposed for S3's `check-toolchain`. When the toolchain is
     missing, fall back to JS with one warning; `--require-wasm` turns that into an error.
4. **JS glue generator**, given an `IslandSpec`, produces an ESM module that:
   - instantiates the `.wasm` (top-level await by default; with `glue: "ready"`, exports a
     `ready()` promise instead)
   - exports wrappers with the original JS signatures that marshal arguments and copy back
   - works both in Node and with a Vite-emitted asset URL
   - stays small and readable

   Snapshot the glue output.
5. **Hand-written islands** in `tests/islands/`. Each one is a TS fixture, a hand-written
   Rust crate in the expected generated style, and its `IslandSpec`. Cover:
   - scalar maths (nbody step)
   - a `Float64Array` in-place transform (an image filter on a `Uint8ClampedArray`)
   - `number[]` reduction
   - integer-proven loops with bitwise ops
   - a plain-data class used internally
   - multi-function islands with internal calls

   Wire leg (c), JS+WASM, of the differential runner to use these, so each fixture's
   `main()` produces identical output with the WASM island swapped in.
6. **Facade integration:** `BuildOutput.islands` carries the wasm bytes + glue, and
   `--emit=wasm` / `--emit=rust` work for islands supplied this way, behind a test-only or
   debug hook. Real island selection arrives with S6/S8.

## Acceptance
- The differential suite's WASM leg passes on every hand-written island.
- The golden numeric tables pass.
- `docs/ABI.md` is complete enough that someone writing S6 could generate conforming Rust
  from it alone.
- A cached rebuild of an unchanged island does no cargo invocation, and a test verifies
  this.
- Missing-toolchain fallback is tested by simulating an absent `cargo` on PATH.

## Out of scope
Lowering TS to Rust (S6), deciding which functions become islands (S8), the Vite plugin
(S3; just give it what it needs through `BuildOutput`).
