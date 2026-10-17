// Stages and publishes the npm packages: one `@trioxide/binding-<platform>` per napi target
// (holding the prebuilt `.node`) and `trioxide` itself, which lists them as optional
// dependencies so npm installs only the one matching the host (the oxlint/rolldown pattern).
//
//   node scripts/publish.js --version 1.2.3 --artifacts <dir> --out <dir> [--dry-run]
//     [--allow-missing] [--no-npm] [--tag <dist-tag>] [--provenance]
//
// `--artifacts` holds the built bindings, found by file name (`trioxide.<platform>.node`) at any
// depth. Packages are staged under `--out` so the checkout is never modified, then published with
// `npm publish` (`--dry-run` only packs). `--no-npm` stops after staging.

import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';

const packageDir = fileURLToPath(new URL('..', import.meta.url));

/** The files of the `trioxide` package, besides package.json. */
const MAIN_FILES = ['bin', 'binding/index.cjs', 'binding/index.d.cts', 'src', 'schema.json', 'README.md'];

export function stage({ version, artifacts, out, allowMissing = false }) {
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) throw new Error(`not a version: ${version}`);
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  const built = findBindings(artifacts);

  const platforms = [];
  for (const dir of readdirSync(path.join(packageDir, 'npm')).sort()) {
    const manifest = readJson(path.join(packageDir, 'npm', dir, 'package.json'));
    const binary = manifest.main;
    const staged = path.join(out, 'npm', dir);
    mkdirSync(staged, { recursive: true });
    cpSync(path.join(packageDir, 'npm', dir, 'README.md'), path.join(staged, 'README.md'));
    writeJson(path.join(staged, 'package.json'), { ...manifest, version });
    if (built.has(binary)) {
      cpSync(built.get(binary), path.join(staged, binary));
    } else if (!allowMissing) {
      throw new Error(`no ${binary} under ${artifacts}`);
    }
    platforms.push({ name: manifest.name, dir: staged, present: built.has(binary) });
  }

  const manifest = readJson(path.join(packageDir, 'package.json'));
  const { devDependencies: _dev, scripts: _scripts, napi: _napi, ...published } = manifest;
  published.version = version;
  published.optionalDependencies = Object.fromEntries(platforms.map(({ name }) => [name, version]));
  const main = path.join(out, 'trioxide');
  for (const file of MAIN_FILES) {
    if (existsSync(path.join(packageDir, file))) {
      cpSync(path.join(packageDir, file), path.join(main, file), { recursive: true });
    }
  }
  cpSync(path.join(packageDir, '../../LICENSE'), path.join(main, 'LICENSE'));
  writeJson(path.join(main, 'package.json'), published);
  return { main, platforms };
}

export function publish({ main, platforms }, { dryRun, tag, provenance }) {
  const args = ['publish', '--access', 'public'];
  if (dryRun) args.push('--dry-run');
  if (tag) args.push('--tag', tag);
  if (provenance && !dryRun) args.push('--provenance');
  // Platform packages first, so `trioxide` never points at a missing optional dependency.
  for (const { dir, present } of platforms) {
    if (present) execFileSync('npm', args, { cwd: dir, stdio: 'inherit' });
  }
  execFileSync('npm', args, { cwd: main, stdio: 'inherit' });
}

function findBindings(dir) {
  const found = new Map();
  if (!dir || !existsSync(dir)) return found;
  for (const entry of readdirSync(dir, { recursive: true, withFileTypes: true })) {
    if (entry.isFile() && /^trioxide\..+\.node$/.test(entry.name)) {
      found.set(entry.name, path.join(entry.parentPath, entry.name));
    }
  }
  return found;
}

function readJson(file) {
  return JSON.parse(readFileSync(file, 'utf8'));
}

function writeJson(file, value) {
  writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { values } = parseArgs({
    options: {
      version: { type: 'string' },
      artifacts: { type: 'string' },
      out: { type: 'string', default: path.join(packageDir, '.publish') },
      'dry-run': { type: 'boolean', default: false },
      'allow-missing': { type: 'boolean', default: false },
      'no-npm': { type: 'boolean', default: false },
      tag: { type: 'string' },
      provenance: { type: 'boolean', default: false },
    },
  });
  if (!values.version) throw new Error('--version is required');
  const staged = stage({
    version: values.version.replace(/^v/, ''),
    artifacts: values.artifacts,
    out: path.resolve(values.out),
    allowMissing: values['allow-missing'],
  });
  for (const { name, present } of staged.platforms) console.log(`${present ? 'staged' : 'missing'} ${name}`);
  if (!values['no-npm']) {
    publish(staged, { dryRun: values['dry-run'], tag: values.tag, provenance: values.provenance });
  }
}
