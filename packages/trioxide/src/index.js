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
 * Builds `entries` and returns the decision log for `target` (`file` or `file:function`).
 * @param {string[]} entries
 * @param {string} [target]
 * @param {TrioxideOptions & { root?: string }} [options]
 */
export function explain(entries, target, options = {}) {
  const { root, ...rest } = options;
  const project = new binding.Project(path.resolve(root ?? process.cwd()), rest);
  project.build(entries.map((entry) => path.resolve(entry)));
  return project.explain(target ?? null);
}

/** The JSON schema of `trioxide.json`. */
export function optionsSchema() {
  return binding.optionsSchema();
}

/** Whether the loaded binding embeds the tsrs type checker. */
export const hasTsrs = binding.hasTsrs();
