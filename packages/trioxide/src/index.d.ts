import type {
  BuildOutput,
  Diagnostic,
  Invalidation,
  IslandArtifact,
  IslandRef,
  Label,
  ModuleOutput,
} from '../binding/index.cjs';

export type { BuildOutput, Diagnostic, Invalidation, IslandArtifact, IslandRef, Label, ModuleOutput };

/** Options, in the shape of `trioxide.json` (see `schema/trioxide.schema.json`). */
export interface TrioxideOptions {
  /** The JSON schema reference; ignored. */
  $schema?: string;
  /** How aggressively functions move to WebAssembly. Default `"auto"`. */
  mode?: 'js-only' | 'auto' | 'wasm-preferred';
  /** Closed world (`app`) or open world (`library`). */
  world?: 'app' | 'library';
  /** Treat declared TypeScript types as proof. Default `false`. */
  trustTypes?: boolean;
  /** Report TypeScript type errors. Default `false`. */
  check?: boolean;
  /** The tsconfig to use; default the nearest one above the root. */
  tsconfig?: string;
  /** Override the tsconfig's `target`. */
  target?: string;
  /** Generate sourcemaps. Default `true`. */
  sourcemap?: boolean;
  /** Turn passes on or off by name. */
  passes?: Record<string, boolean>;
  /** WebAssembly output. */
  wasm?: {
    enabled?: boolean;
    require?: boolean;
    wasmOpt?: boolean;
    glue?: 'tla' | 'ready';
  };
  /** Vite dev-server settings. */
  dev?: { wasm?: boolean };
  /** Where to cache compiled islands. */
  cacheDir?: string;
  /** Per-file overrides, later entries win. */
  overrides?: Array<{ files: string[]; target: 'js' | 'wasm' }>;
}

/** Per function: JS or WASM, and why. */
export interface FunctionDecision {
  name: string | null;
  span: { start: number; end: number };
  target: 'js' | 'wasm';
  reason: string;
  culprit?: { start: number; end: number };
  island?: string;
  cost?: { benefit: number; cost: number; threshold: number };
}

/** One pass decision at one site. */
export interface PassEvent {
  pass: string;
  span?: { start: number; end: number };
  kind: 'applied' | 'skipped';
  reason?: string;
  message?: string;
}

/** The decision log of a build. */
export interface Report {
  typeOracle: string;
  modules: Array<{ path: string; functions?: FunctionDecision[]; passes?: PassEvent[] }>;
  diagnostics?: Diagnostic[];
}

/** A trioxide project: the module graph plus caches, kept alive across calls. */
export declare class Project {
  constructor(root: string, options?: TrioxideOptions | null);
  /** The canonical project root. */
  get root(): string;
  /** Compiles the whole program reachable from `entries`. */
  build(entries: string[]): BuildOutput & { report: Report };
  /** Records new contents for `path`, or re-reads it from disk when `source` is omitted. */
  update(path: string, source?: string | null): Invalidation;
  /** The compiled form of one module. */
  transform(path: string): ModuleOutput;
  /** The decision log for `target` (`file` or `file:function`; everything if omitted). */
  explain(target?: string | null): Report;
}

/** Transforms one module from source. `filename` need not exist on disk. */
export declare function transform(code: string, filename: string, options?: TrioxideOptions | null): ModuleOutput;

/** Compiles the program reachable from `entries`. */
export declare function build(
  entries: string[],
  options?: TrioxideOptions & { root?: string },
): BuildOutput & { report: Report };

/** Builds `entries` and returns the decision log for `target`. */
export declare function explain(
  entries: string[],
  target?: string,
  options?: TrioxideOptions & { root?: string },
): Report;

/** The JSON schema of `trioxide.json`. */
export declare function optionsSchema(): Record<string, unknown>;

/** Whether the loaded binding embeds the tsrs type checker. */
export declare const hasTsrs: boolean;
