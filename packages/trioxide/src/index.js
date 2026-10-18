// The trioxide JavaScript API: a thin, typed layer over the napi binding (see index.d.ts).

import { createRequire } from 'node:module';
import path from 'node:path';

const require = createRequire(import.meta.url);
const binding = require('../binding/index.cjs');

/** @typedef {import('./index.d.ts').TrioxideOptions} TrioxideOptions */

export const Project = binding.Project;

/**
 * Transforms one module from source.
 * @param {string} code
 * @param {string} filename
 * @param {TrioxideOptions} [options]
 */
export function transform(code, filename, options) {
  return binding.transform(code, filename, options ?? null);
}

/**
 * Compiles the program reachable from `entries`.
 * @param {string[]} entries
 * @param {TrioxideOptions & { root?: string }} [options]
 */
export function build(entries, options = {}) {
  const { root, ...rest } = options;
  const project = new binding.Project(path.resolve(root ?? process.cwd()), rest);
  return project.build(entries.map((entry) => path.resolve(entry)));
}

/**
 * Plans the program reachable from `entries` (analysis, partitioning and the JS passes, no
 * island builds) and returns the decision log for `target` (`file` or `file:function`;
 * every module if omitted): per function, JS or WASM and why, the failed eligibility rules,
 * the cost-model numbers, and the passes that fired. `trioxide explain --json` prints the same.
 * @param {string[]} entries
 * @param {string} [target]
 * @param {TrioxideOptions & { root?: string }} [options]
 */
export function explain(entries, target, options = {}) {
  const { root, ...rest } = options;
  const project = new binding.Project(path.resolve(root ?? process.cwd()), rest);
  project.plan(entries.map((entry) => path.resolve(entry)));
  return project.explain(target ?? null);
}

/** The JSON schema of `trioxide.json`. */
export function optionsSchema() {
  return binding.optionsSchema();
}

/** The string island glue uses in place of its `.wasm` URL; emitters replace it. */
export const WASM_URL_PLACEHOLDER = binding.WASM_URL_PLACEHOLDER;

/** Whether the loaded binding embeds the tsrs type checker. */
export const hasTsrs = binding.hasTsrs();

/**
 * The nearest `trioxide.json` / `trioxide.jsonc` at or above `dir`, or `null`.
 * @param {string} dir
 */
export function findConfig(dir) {
  return binding.findConfig(path.resolve(dir)) ?? null;
}

/**
 * Reads and validates a config file; returns the options as written, without defaults.
 * @param {string} file
 * @returns {TrioxideOptions}
 */
export function loadConfig(file) {
  return binding.loadConfig(path.resolve(file));
}

/**
 * Runs the `trioxide` command line in this process and returns its exit code.
 * @param {string[]} argv the arguments, without the program name
 */
export function runCli(argv) {
  return binding.runCli(['trioxide', ...argv]);
}
