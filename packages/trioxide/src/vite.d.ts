import type { Plugin } from 'vite';

import type { Project, TrioxideOptions } from './index.d.ts';

/** Options of the trioxide Vite plugin: `trioxide.json` options plus plugin settings. */
export interface TrioxidePluginOptions extends TrioxideOptions {
  /**
   * The `trioxide.json` / `trioxide.jsonc` to merge these options over (they win), relative
   * to the Vite root; `false` to ignore config files. Default: the nearest one at or above
   * the Vite root, stopping at the repository root.
   */
  configFile?: string | false;
}

/** What the plugin exposes to other plugins as `plugin.api`. */
export interface TrioxidePluginApi {
  /** The project, once the build or dev server has started. */
  readonly project: Project | undefined;
  /** The options after merging `trioxide.json` and applying the Vite defaults. */
  readonly options: TrioxideOptions;
}

/**
 * The trioxide Vite plugin. It compiles `.ts/.tsx/.mts/.cts` modules before Vite's own
 * transform (`enforce: 'pre'`), runs whole-program analysis at build start, serves WASM island
 * glue as `\0trioxide:island/<id>` modules and emits their `.wasm` files as hashed assets.
 *
 * Builds default to `world: "app"` (`"library"` with `build.lib`). The dev server compiles
 * incrementally and only builds WASM islands with `dev: { wasm: true }`.
 */
export default function trioxide(options?: TrioxidePluginOptions): Plugin & { api: TrioxidePluginApi };
