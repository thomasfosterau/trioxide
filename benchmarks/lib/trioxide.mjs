// Building benchmark programs with trioxide: the binary, and the variants every suite compares.

import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const benchmarks = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const repo = path.resolve(benchmarks, '..');
export const out = path.join(benchmarks, '.out');
export const cacheDir = path.join(out, '.cache');

/**
 * The trioxide binary: TRIOXIDE_BIN, else target/release, else target/debug (built first
 * when neither exists).
 */
export function binary() {
  if (process.env.TRIOXIDE_BIN) return process.env.TRIOXIDE_BIN;
  const found = ['release', 'debug'].map((p) => path.join(repo, 'target', p, 'trioxide')).find((p) => existsSync(p));
  if (found) return found;
  execFileSync('cargo', ['build', '--quiet', '--release', '-p', 'trioxide_cli'], { cwd: repo, stdio: 'inherit' });
  return path.join(repo, 'target', 'release', 'trioxide');
}

/** An empty output directory that Node treats as ESM. */
export function prepare(dir) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  writeFileSync(path.join(dir, 'package.json'), '{ "type": "module" }\n');
}

/** The TypeScript sources under `root` (relative paths), skipping node_modules and dist. */
export function sources(root, dir = '') {
  const found = [];
  for (const name of readdirSync(path.join(root, dir)).sort()) {
    const relative = path.join(dir, name);
    if (name === 'node_modules' || name === 'dist' || name.startsWith('.')) continue;
    if (statSync(path.join(root, relative)).isDirectory()) found.push(...sources(root, relative));
    else if (/\.tsx?$/.test(name) && !name.endsWith('.d.ts') && !name.startsWith('vite.config')) found.push(relative);
  }
  return found;
}

export function jsName(file) {
  return file.replace(/\.tsx?$/, '.js');
}

/** Types stripped only (`trioxide strip`), every source of `root` into `dir`. */
export function strip(trioxide, root, dir) {
  prepare(dir);
  for (const file of sources(root)) {
    const target = path.join(dir, jsName(file));
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, execFileSync(trioxide, ['strip', '--root', root, path.join(root, file)], { encoding: 'utf8' }));
  }
}

/**
 * `trioxide build` of `entry` (relative to `root`) into `dir`, with `options` merged over the
 * program's own trioxide.json (whose `passes` stay unless `options.passes` replaces them).
 * Returns the island glue modules.
 */
export function build(trioxide, root, entry, dir, options, own = {}) {
  prepare(dir);
  const config = `${dir}.json`;
  writeFileSync(config, JSON.stringify({ ...own, cacheDir, ...options }));
  const args = ['build', '--config', config, '--root', root, '--out-dir', dir, path.join(root, entry)];
  if (options.wasm?.enabled !== false) args.splice(1, 0, '--require-wasm');
  const result = spawnSync(trioxide, args, { encoding: 'utf8' });
  if (result.status !== 0) throw new Error(`trioxide ${args.join(' ')} failed:\n${result.stderr}`);
  const islands = path.join(dir, '_islands');
  return existsSync(islands)
    ? readdirSync(islands).filter((name) => name.endsWith('.js')).map((name) => path.join(islands, name))
    : [];
}

/** Every pass on, including the off-by-default ones (`specialize` stays off). */
export const ALL_PASSES = { 'inline-fn': true, scalar: true };
