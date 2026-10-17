// The implementation of `trioxide/vite` (see vite.d.ts for the options). `createPlugin` takes
// the Project constructor as a parameter so tests can wrap it, e.g. to inject a fake island.

import { readFileSync, realpathSync } from 'node:fs';
import path from 'node:path';

import { WASM_URL_PLACEHOLDER, findConfig, loadConfig } from './index.js';

const TS_RE = /\.(?:[mc]?ts|tsx)$/;
/** Queries whose modules are not the file's code (Vite turns them into JS strings). */
const NON_CODE_QUERY_RE = /[?&](?:raw|url|inline|no-inline)(?:&|=|$)/;
const ISLAND_PREFIX = '\0trioxide:island/';

/**
 * @typedef {import('./vite.d.ts').TrioxidePluginOptions} TrioxidePluginOptions
 * @typedef {import('./index.d.ts').TrioxideOptions} TrioxideOptions
 * @typedef {import('./index.d.ts').Project} Project
 * @typedef {import('./index.d.ts').ModuleOutput} ModuleOutput
 * @typedef {import('./index.d.ts').Diagnostic} Diagnostic
 */

/**
 * @param {TrioxidePluginOptions} [pluginOptions]
 * @param {{ createProject: (root: string, options: TrioxideOptions) => Project }} hooks
 * @returns {import('vite').Plugin}
 */
export function createPlugin(pluginOptions = {}, { createProject }) {
  /** @type {import('vite').ResolvedConfig} */
  let config;
  /** @type {TrioxideOptions} */
  let options;
  /** @type {Project | undefined} */
  let project;
  let isBuild = false;
  /** Whole-program build output by canonical path; entries are dropped when a file changes. */
  const built = new Map();
  /** Island id → { wasm, glue } from the last whole-program build. */
  const islands = new Map();
  /** Code last served per canonical path (dev only), to see which dependants changed. */
  const served = new Map();

  function getProject() {
    project ??= createProject(config.root, options);
    return project;
  }

  /** The output for `file` given its current `code`, from the whole-program build if fresh. */
  function compile(file, code) {
    const prebuilt = built.get(canonical(file));
    if (prebuilt && code === readOrEmpty(file)) return prebuilt;
    const p = getProject();
    p.update(file, code);
    return p.transform(file);
  }

  return {
    name: 'trioxide',
    enforce: 'pre',

    config(userConfig) {
      // trioxide returns JS for every TypeScript module, so Vite's own oxc transform must not
      // run on them again (it would re-parse plain JS as TypeScript).
      if (userConfig.oxc === false) return undefined;
      return { oxc: { exclude: [TS_RE] } };
    },

    configResolved(resolved) {
      config = resolved;
      isBuild = resolved.command === 'build';
      options = resolveOptions(pluginOptions, resolved);
    },

    buildStart(inputOptions) {
      built.clear();
      islands.clear();
      served.clear();
      project = undefined;
      try {
        getProject();
      } catch (error) {
        this.error(`trioxide: ${error.message}`);
      }
      const devWasm = !isBuild && options.wasm?.enabled !== false;
      if (!isBuild && !devWasm) return;

      const input = inputOptions.input ?? path.join(config.root, 'index.html');
      const entries = collectEntries(input, config.root);
      if (entries.length === 0) return;
      let output;
      try {
        output = getProject().build(entries);
      } catch (error) {
        this.error(`trioxide: ${error.message}`);
      }
      for (const diagnostic of output.report.diagnostics ?? []) {
        if (diagnostic.severity === 'error') this.error(toRollupError(diagnostic));
        else this.warn(toRollupError(diagnostic));
      }
      for (const module of output.modules) built.set(canonical(module.path), module);
      for (const island of output.islands) islands.set(island.id, island);
    },

    resolveId(id) {
      if (id.startsWith(ISLAND_PREFIX)) return id;
      if (id.startsWith(ISLAND_PREFIX.slice(1))) return `\0${id}`;
      return undefined;
    },

    load(id) {
      if (!id.startsWith(ISLAND_PREFIX)) return undefined;
      const islandId = id.slice(ISLAND_PREFIX.length);
      const island = islands.get(islandId);
      if (!island) this.error(`trioxide: unknown island "${islandId}"`);
      // Dev inlines the module: Vite leaves `new URL("data:…", import.meta.url)` alone.
      const url = isBuild ? wasmToken(islandId) : `data:application/wasm;base64,${Buffer.from(island.wasm).toString('base64')}`;
      return { code: island.glue.replaceAll(WASM_URL_PLACEHOLDER, url), moduleType: 'js' };
    },

    transform(code, id) {
      if (id.startsWith('\0') || NON_CODE_QUERY_RE.test(id)) return undefined;
      const file = cleanUrl(id);
      if (!TS_RE.test(file)) return undefined;

      let out;
      try {
        out = compile(file, code);
      } catch (error) {
        this.error(`trioxide: ${error.message}`);
      }
      const source = code;
      let firstError;
      for (const diagnostic of out.diagnostics) {
        const sourceOf = diagnostic.file && canonical(diagnostic.file) !== canonical(file) ? undefined : source;
        const error = toRollupError(diagnostic, sourceOf ?? readOrEmpty(diagnostic.file));
        if (diagnostic.severity === 'error') firstError ??= error;
        else this.warn(error);
      }
      if (firstError) this.error(firstError);
      if (!isBuild) served.set(canonical(file), out.code);
      return { code: out.code, map: out.map ? JSON.parse(out.map) : null, moduleType: 'js' };
    },

    async hotUpdate({ file, type, read, modules }) {
      if (type === 'delete' || !TS_RE.test(file) || !project) return undefined;
      const invalidation = project.update(file, await read());
      for (const stale of [file, ...invalidation.dependants]) built.delete(canonical(stale));
      const changed = [];
      for (const dependant of invalidation.dependants) {
        const key = canonical(dependant);
        if (!served.has(key)) continue;
        let out;
        try {
          out = project.transform(dependant);
        } catch {
          continue;
        }
        if (out.code === served.get(key)) continue;
        for (const module of this.environment.moduleGraph.getModulesByFile(dependant) ?? []) {
          this.environment.moduleGraph.invalidateModule(module);
          changed.push(module);
        }
      }
      if (changed.length === 0) return undefined;
      return [...new Set([...modules, ...changed])];
    },

    generateBundle(_outputOptions, bundle) {
      const files = new Map();
      for (const [id, island] of islands) {
        const token = wasmToken(id);
        const chunks = Object.values(bundle).filter((chunk) => chunk.type === 'chunk' && chunk.code.includes(token));
        if (chunks.length === 0) continue;
        if (!files.has(id)) {
          const ref = this.emitFile({ type: 'asset', name: `${id}.wasm`, source: island.wasm });
          files.set(id, this.getFileName(ref));
        }
        for (const chunk of chunks) {
          let relative = path.posix.relative(path.posix.dirname(chunk.fileName), files.get(id));
          if (!relative.startsWith('.')) relative = `./${relative}`;
          chunk.code = chunk.code.replaceAll(token, relative);
        }
      }
    },

    api: {
      /** The current Project, if one has been created. */
      get project() {
        return project;
      },
      /** The resolved trioxide options. */
      get options() {
        return options;
      },
    },
  };
}

/**
 * Merges `trioxide.json` with the plugin options (plugin options win) and applies the Vite
 * defaults: `world: "app"` (or `"library"` for `build.lib`), and no WASM in dev unless
 * `dev.wasm` is set.
 * @param {TrioxidePluginOptions} pluginOptions
 * @param {import('vite').ResolvedConfig} config
 * @returns {TrioxideOptions}
 */
export function resolveOptions(pluginOptions, config) {
  const { configFile, ...inline } = pluginOptions;
  let fileOptions = {};
  if (configFile !== false) {
    const file = configFile ? path.resolve(config.root, configFile) : findConfig(config.root);
    if (file) {
      fileOptions = loadConfig(file);
      for (const key of ['tsconfig', 'cacheDir']) {
        if (typeof fileOptions[key] === 'string') {
          fileOptions[key] = path.resolve(path.dirname(file), fileOptions[key]);
        }
      }
    }
  }
  const options = merge(fileOptions, inline);
  options.world ??= config.build?.lib ? 'library' : 'app';
  if (config.command !== 'build' && !options.dev?.wasm) {
    options.wasm = { ...options.wasm, enabled: false };
  }
  return options;
}

function merge(base, override) {
  const out = { ...base };
  for (const [key, value] of Object.entries(override)) {
    if (value === undefined) continue;
    out[key] = isPlainObject(value) && isPlainObject(out[key]) ? merge(out[key], value) : value;
  }
  return out;
}

function isPlainObject(value) {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * TypeScript entry modules from Rollup's `input`: TS inputs directly, and the module scripts
 * of HTML inputs (`<script type="module" src="…">`).
 * @param {import('rollup').InputOption | undefined} input
 * @param {string} root
 */
export function collectEntries(input, root) {
  const inputs = input === undefined ? [] : typeof input === 'string' ? [input] : Object.values(input);
  const entries = [];
  for (const item of inputs) {
    const file = path.resolve(root, item);
    if (TS_RE.test(file)) {
      entries.push(file);
    } else if (file.endsWith('.html')) {
      let html;
      try {
        html = readFileSync(file, 'utf8');
      } catch {
        continue;
      }
      for (const match of html.matchAll(/<script\b[^>]*>/gi)) {
        const tag = match[0];
        if (!/\btype\s*=\s*["']?module\b/i.test(tag)) continue;
        const src = /\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/i.exec(tag);
        const value = src && (src[1] ?? src[2] ?? src[3]);
        if (!value || /^[a-z]+:/i.test(value) || value.startsWith('//')) continue;
        const clean = cleanUrl(value);
        const resolved = clean.startsWith('/') ? path.join(root, clean) : path.resolve(path.dirname(file), clean);
        if (TS_RE.test(resolved)) entries.push(resolved);
      }
    }
  }
  return [...new Set(entries)];
}

/**
 * A trioxide diagnostic as a Rollup error/warning, with location and code frame, which Vite
 * shows in the terminal and its error overlay.
 * @param {Diagnostic} diagnostic
 * @param {string} [source] the text the diagnostic's spans point into
 */
export function toRollupError(diagnostic, source = '') {
  const code = diagnostic.code ? `[${diagnostic.code}] ` : '';
  const help = diagnostic.help ? `\nhelp: ${diagnostic.help}` : '';
  const error = { message: `${code}${diagnostic.message}${help}`, plugin: 'trioxide' };
  if (diagnostic.file) error.id = diagnostic.file;
  const label = diagnostic.labels[0];
  if (label && source) {
    const start = position(source, label.start);
    const end = position(source, Math.max(label.end, label.start));
    error.loc = { file: diagnostic.file, line: start.line, column: start.column };
    error.frame = codeFrame(source, start, end, label.message);
  }
  return error;
}

/** Line (1-based) and column (0-based, UTF-16) of a UTF-8 byte offset. */
function position(source, byteOffset) {
  const before = Buffer.from(source).subarray(0, byteOffset).toString();
  const lines = before.split('\n');
  return { line: lines.length, column: lines.at(-1).length };
}

/** A small code frame: two lines of context, the range underlined. */
export function codeFrame(source, start, end, message) {
  const lines = source.split(/\r?\n/);
  const first = Math.max(1, start.line - 2);
  const last = Math.min(lines.length, end.line + 2);
  const width = String(last).length;
  const out = [];
  for (let line = first; line <= last; line++) {
    const text = lines[line - 1];
    const marked = line >= start.line && line <= end.line;
    out.push(`${marked ? '>' : ' '} ${String(line).padStart(width)} | ${text}`);
    if (!marked) continue;
    const from = line === start.line ? start.column : 0;
    const to = line === end.line ? end.column : text.length;
    const underline = `${' '.repeat(from)}${'^'.repeat(Math.max(1, to - from))}`;
    const note = line === end.line && message ? ` ${message}` : '';
    out.push(`  ${' '.repeat(width)} | ${underline}${note}`);
  }
  return out.join('\n');
}

function wasmToken(islandId) {
  return `__TRIOXIDE_WASM_${Buffer.from(islandId).toString('hex')}__`;
}

function cleanUrl(url) {
  return url.replace(/[?#].*$/s, '');
}

function canonical(file) {
  try {
    return realpathSync(file);
  } catch {
    return path.resolve(file);
  }
}

function readOrEmpty(file) {
  try {
    return file ? readFileSync(file, 'utf8') : '';
  } catch {
    return '';
  }
}
