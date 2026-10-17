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

import type { TrioxideOptions } from './options.d.ts';

export type * from './options.d.ts';

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

/**
 * The string island glue uses in place of its `.wasm` URL, as in
 * `new URL("__TRIOXIDE_WASM_URL__", import.meta.url)`; emitters replace it.
 */
export declare const WASM_URL_PLACEHOLDER: string;

/** Whether the loaded binding embeds the tsrs type checker. */
export declare const hasTsrs: boolean;

/**
 * The nearest `trioxide.json` / `trioxide.jsonc` at or above `dir` (stopping at the
 * repository root), or `null`.
 */
export declare function findConfig(dir: string): string | null;

/** Reads and validates a config file; returns the options as written, without defaults. */
export declare function loadConfig(path: string): TrioxideOptions;

/**
 * Runs the `trioxide` command line (`argv` without the program name) in this process and
 * returns its exit code. Output goes to the process's stdout and stderr.
 */
export declare function runCli(argv: string[]): number;
