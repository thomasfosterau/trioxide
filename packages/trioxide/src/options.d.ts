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
   * Prefer WASM for every eligible function unless the cost model strongly objects.
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
 * Forces files matching `files` to stay JS or to become WASM.
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
