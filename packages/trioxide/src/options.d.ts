// Generated from schema/trioxide.schema.json by scripts/generate-types.js. Do not edit.

/**
 * Everything that configures a trioxide build.
 *
 * Read from `trioxide.json` / `trioxide.jsonc` (see `Options::from_file`), merged with Vite
 * plugin options or CLI flags. Source directives (`"use wasm"` / `"use js"`,
 * `/** @trioxide wasm|js *\/`) override all of it. Field names are camelCase in JSON. The JSON
 * schema in `schema/trioxide.schema.json` is generated from this type.
 */
export interface TrioxideOptions {
  /**
   * The JSON schema reference, allowed so editors can validate config files. Ignored.
   */
  "$schema"?: string | null;
  /**
   * How aggressively functions move to WebAssembly.
   *
   * @default "auto"
   */
  mode?: Mode;
  /**
   * Whether the whole program is visible (`app`) or exports may be called from unknown code
   * (`library`). Unset: `app` when building from entries (Vite builds, `trioxide build`),
   * `library` for single-file transforms.
   */
  world?: World | null;
  /**
   * Treat TypeScript types that rest on annotations, casts or `any` as proof. Unsound if
   * the types lie; off by default.
   *
   * @default false
   */
  trustTypes?: boolean;
  /**
   * Report TypeScript type errors as diagnostics.
   *
   * @default false
   */
  check?: boolean;
  /**
   * The `tsconfig.json` to use. Unset: the nearest one above the project root.
   */
  tsconfig?: string | null;
  /**
   * Override the tsconfig's `target` (e.g. `"es2022"`, `"esnext"`).
   */
  target?: string | null;
  /**
   * Generate sourcemaps.
   *
   * @default true
   */
  sourcemap?: boolean;
  /**
   * Turn individual optimisation passes on or off by name, e.g. `{ "fold": false }`. Passes
   * not listed use their defaults. `"specialize": true` also lets passes fold facts that
   * hold only because every caller passes the same values into function bodies (off by
   * default: it makes output harder to read).
   *
   * @default {}
   */
  passes?: Record<string, boolean>;
  /**
   * WebAssembly output.
   *
   * @default {"enabled":true,"require":false,"wasmOpt":true,"glue":"ready"}
   */
  wasm?: WasmOptions;
  /**
   * Settings that apply only to the Vite dev server.
   *
   * @default {"wasm":false}
   */
  dev?: DevOptions;
  /**
   * Where to cache compiled islands. Unset: `node_modules/.trioxide` under the root, or the
   * system cache directory.
   */
  cacheDir?: string | null;
  /**
   * Per-file overrides, applied in order (later wins).
   */
  overrides?: Array<Override>;
  /**
   * The partitioner's cost model: when moving a cluster of functions to WebAssembly pays
   * (`trioxide explain` shows its numbers). Fields not given keep their defaults.
   *
   * @default {"threshold":0,"wasmPreferredThreshold":-1000000,"arithmetic":-0.021,"integer":-0.0595,"conversion":-11.7,"wrapping":3.65,"math":0.0242,"remainder":-1.22,"index":0.245,"checkedIndex":-7.7,"iteration":0.333,"trips":256,"crossing":13.1,"aggregate":45.9,"byte":0.169,"numberArray":12,"instantiation":300000}
   */
  costModel?: CostModel;
}

/**
 * How aggressively functions move to WebAssembly.
 */
export type Mode =
  /**
   * Never create WASM islands; only optimise JS.
   */
  | "js-only"
  /**
   * Create islands the cost model says are profitable.
   */
  | "auto"
  /**
   * Like `auto`, with the cost model's lower `wasmPreferredThreshold`: WASM for every
   * eligible function unless the cost model strongly objects.
   */
  | "wasm-preferred";

/**
 * The world assumption: what code outside the module graph may do.
 */
export type World =
  /**
   * Closed world: the whole module graph is visible and exports are only used internally.
   */
  | "app"
  /**
   * Open world: exported functions may be called with anything their types allow.
   */
  | "library";

/**
 * WebAssembly settings.
 */
export interface WasmOptions {
  /**
   * Produce WASM islands at all. `--no-wasm` turns this off.
   *
   * @default true
   */
  enabled?: boolean;
  /**
   * Fail the build if the Rust toolchain is missing instead of falling back to JS.
   *
   * @default false
   */
  require?: boolean;
  /**
   * Run `wasm-opt` on islands when it is on PATH.
   *
   * @default true
   */
  wasmOpt?: boolean;
  /**
   * How island glue waits for instantiation.
   *
   * @default "ready"
   */
  glue?: GlueMode;
}

/**
 * How island glue modules wait for their `.wasm` to instantiate.
 */
export type GlueMode =
  /**
   * Top-level `await` in the glue module: islands run from the first call, but modules
   * importing them finish evaluating later (after other pending tasks).
   */
  | "tla"
  /**
   * No top-level `await`: functions run their JS versions until the island is
   * instantiated, and the glue exports a `ready()` promise. Module evaluation order is
   * unchanged.
   */
  | "ready";

/**
 * Vite dev-server settings.
 */
export interface DevOptions {
  /**
   * Build WASM islands in dev too (off by default for fast reloads).
   *
   * @default false
   */
  wasm?: boolean;
}

/**
 * Forces files matching `files` to stay JS or to become WASM: like a `"use js"` or
 * `"use wasm"` file directive (directives in the file win). Overrides apply even in
 * `js-only` mode, but not with WASM off.
 */
export interface Override {
  /**
   * Glob patterns, relative to the project root.
   */
  files: Array<string>;
  /**
   * What to force.
   */
  target: OverrideTarget;
}

/**
 * The target an `Override` forces.
 */
export type OverrideTarget =
  /**
   * Like `"use js"` on every function.
   */
  | "js"
  /**
   * Like `"use wasm"` on every function; ineligible functions are errors.
   */
  | "wasm";

/**
 * The cost model's weights and thresholds (`costModel` in `trioxide.json`).
 *
 * Operation weights are the nanoseconds WASM saves per operation compared with V8's
 * optimised JS (negative where the island is slower); the cost terms are nanoseconds too.
 * The defaults are fitted to `benchmarks/partition/` (Node 22, V8 12.4) for the code the
 * Rust backend generates today.
 */
export interface CostModel {
  /**
   * In `auto` mode an island is kept when `benefit − cost` exceeds this (nanoseconds).
   *
   * @default 0
   */
  threshold?: number;
  /**
   * The threshold in `wasm-preferred` mode.
   *
   * @default -1000000
   */
  wasmPreferredThreshold?: number;
  /**
   * Nanoseconds saved per floating-point arithmetic operation or comparison.
   *
   * @default -0.021
   */
  arithmetic?: number;
  /**
   * Nanoseconds saved per operation on values the island holds as int32s (arithmetic,
   * comparisons, `++`, `--`, bitwise operators).
   *
   * @default -0.0595
   */
  integer?: number;
  /**
   * Nanoseconds saved per float operand of a bitwise operator or `Math.imul` (a
   * `ToInt32` in WASM).
   *
   * @default -11.7
   */
  conversion?: number;
  /**
   * Nanoseconds saved per `(a + b) | 0`-style wrapped operation (V8 leaves its int32 fast
   * path when the sum overflows; WASM wraps natively).
   *
   * @default 3.65
   */
  wrapping?: number;
  /**
   * Nanoseconds saved per `Math.*` call or `**`.
   *
   * @default 0.0242
   */
  math?: number;
  /**
   * Nanoseconds saved per `%` on values not proven integers (a call to `fmod` in WASM).
   *
   * @default -1.22
   */
  remainder?: number;
  /**
   * Nanoseconds saved per array element read or write whose index the island computes as
   * an int32.
   *
   * @default 0.245
   */
  index?: number;
  /**
   * Nanoseconds saved per other element read or write (the index is a float, converted
   * and checked on every access).
   *
   * @default -7.7
   */
  checkedIndex?: number;
  /**
   * Nanoseconds saved per loop iteration (its own control flow).
   *
   * @default 0.333
   */
  iteration?: number;
  /**
   * Trip count assumed for loops without a known bound, and the element count assumed
   * for arrays of unknown length crossing the boundary.
   *
   * @default 256
   */
  trips?: number;
  /**
   * Nanoseconds per call into an island (the glue's checks and the arena reset).
   *
   * @default 13.1
   */
  crossing?: number;
  /**
   * Nanoseconds per array or object argument (or result) per call: allocating it in the
   * arena, the glue's aliasing and plain-view checks, the views it creates.
   *
   * @default 45.9
   */
  aggregate?: number;
  /**
   * Nanoseconds per byte copied into or out of linear memory.
   *
   * @default 0.169
   */
  byte?: number;
  /**
   * Multiplier for the bytes of a `number[]`, which is converted element by element.
   *
   * @default 12
   */
  numberArray?: number;
  /**
   * Nanoseconds to instantiate one island.
   *
   * @default 300000
   */
  instantiation?: number;
}
