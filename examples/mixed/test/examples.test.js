// Builds the examples with the Vite plugin, in production and in dev, and checks they compute
// what `trioxide build` (the CLI) builds, with the CLI build's islands ready:
//
// - this app in Chromium, built and served by Vite and by its dev server. The page renders
//   while its module loads, before the island is ready, so this checks the JS the plugin
//   serves (and that the production page loads its island without errors);
// - the CLI examples image-filters and telemetry as an SSR build, with every island ready
//   first, so the islands' WASM runs, and through the dev server's `ssrLoadModule` (WASM is
//   off in dev). The cost model keeps image-filters' filters in JS, so that one is built
//   with every eligible cluster as an island, in both builds.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { build, createServer, preview } from 'vite';
import { afterAll, beforeAll, expect, test } from 'vitest';
import trioxide from 'trioxide/vite';

import { launchChromium } from '../../../packages/trioxide/test/browser.js';

const examples = fileURLToPath(new URL('../..', import.meta.url));
const repo = path.resolve(examples, '..');
const work = path.join(examples, 'mixed', 'node_modules', '.examples-test');

/** TRIOXIDE_BIN, else the debug binary, which cargo brings up to date first. */
function cli() {
  if (process.env.TRIOXIDE_BIN) return process.env.TRIOXIDE_BIN;
  execFileSync('cargo', ['build', '--quiet', '-p', 'trioxide_cli'], { cwd: repo, stdio: 'inherit' });
  return path.join(repo, 'target', 'debug', 'trioxide');
}

/**
 * `run()` of the example built by the CLI (with `options` as its trioxide.json, if given),
 * with every island ready, and the islands' ids.
 */
async function cliRun(name, options) {
  const dir = path.join(work, 'cli', name);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  writeFileSync(path.join(dir, 'package.json'), '{ "type": "module" }\n');
  const root = path.join(examples, name);
  const config = options ? ['--config', path.join(work, 'cli', `${name}.json`)] : [];
  if (options) writeFileSync(config[1], JSON.stringify(options));
  execFileSync(cli(), ['build', ...config, '--root', root, '--out-dir', dir, path.join(root, 'src/main.ts')], { stdio: 'pipe' });
  const module = await import(pathToFileURL(path.join(dir, 'src/main.js')).href);
  const islandsDir = path.join(dir, '_islands');
  const islands = existsSync(islandsDir) ? readdirSync(islandsDir).filter((file) => file.endsWith('.js')).sort() : [];
  for (const glue of islands) await (await import(pathToFileURL(path.join(islandsDir, glue)).href)).ready();
  return { result: module.run(), islands: islands.map((glue) => glue.replace(/\.js$/, '')) };
}

/**
 * Emits every island's glue as a chunk of its own with its exports kept, so the test can
 * await its `ready()` (the app's code imports the same module).
 */
const exposeIslands = {
  name: 'expose-islands',
  moduleParsed(info) {
    if (info.id.startsWith('\0trioxide:island/')) {
      const id = info.id.slice('\0trioxide:island/'.length);
      this.emitFile({ type: 'chunk', id: info.id, name: `island-${id}`, preserveSignature: 'strict' });
    }
  },
};

let browser;

beforeAll(async () => {
  browser = await launchChromium();
});

afterAll(async () => {
  await browser?.close();
});

async function pageLines(url) {
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => message.type() === 'error' && errors.push(message.text()));
  await page.goto(url);
  await page.waitForFunction(() => document.getElementById('app')?.textContent);
  const text = await page.locator('#app').innerText();
  await page.close();
  expect(errors).toEqual([]);
  return text.split('\n');
}

test('mixed renders what the CLI build computes, as Vite builds it and as its dev server serves it', async () => {
  const root = path.join(examples, 'mixed');
  const { result: expected, islands } = await cliRun('mixed');
  expect(islands).toEqual(['kernel']);
  const outDir = path.join(work, 'mixed-dist');
  await build({ root, logLevel: 'error', build: { outDir, emptyOutDir: true } });
  const server = await preview({ root, logLevel: 'silent', build: { outDir }, preview: { host: '127.0.0.1', port: 0 } });
  try {
    expect(await pageLines(server.resolvedUrls.local[0])).toEqual(expected);
  } finally {
    await new Promise((resolve) => server.httpServer.close(resolve));
  }
  const dev = await createServer({ root, logLevel: 'silent', server: { host: '127.0.0.1', port: 0 } });
  await dev.listen();
  try {
    expect(await pageLines(dev.resolvedUrls.local[0])).toEqual(expected);
  } finally {
    await dev.close();
  }
});

const EVERY_CLUSTER = { mode: 'wasm-preferred', costModel: { wasmPreferredThreshold: -1e300 } };

for (const [name, options, expectedIslands] of [
  ['image-filters', EVERY_CLUSTER, ['filters', 'image']],
  ['telemetry', undefined, []],
]) {
  test(`${name} computes the same built by Vite (SSR, islands ready) and through its dev server`, async () => {
    const root = path.join(examples, name);
    const { result: expected, islands } = await cliRun(name, options);
    expect(islands).toEqual(expectedIslands);
    const config = { root, configFile: false, logLevel: 'error', plugins: [trioxide(options), exposeIslands] };
    const outDir = path.join(work, `${name}-dist`);
    // An SSR build doesn't emit assets by default, so the plugin's `.wasm` would be missing and
    // the islands would stay on their JS fallback (reported to S3); emit them to test the WASM.
    await build({ ...config, build: { ssr: 'src/main.ts', outDir, emptyOutDir: true, ssrEmitAssets: true } });
    const assets = path.join(outDir, 'assets');
    const glues = existsSync(assets) ? readdirSync(assets).filter((file) => /^island-.*\.js$/.test(file)).sort() : [];
    expect(glues.map((glue) => glue.replace(/^island-|-[\w-]+\.js$/g, ''))).toEqual(islands);
    for (const glue of glues) await (await import(pathToFileURL(path.join(assets, glue)).href)).ready();
    const built = await import(pathToFileURL(path.join(outDir, 'main.js')).href);
    expect(built.run()).toEqual(expected);
    const server = await createServer({ ...config, plugins: [trioxide(options)], server: { middlewareMode: true } });
    try {
      expect((await server.ssrLoadModule('/src/main.ts')).run()).toEqual(expected);
    } finally {
      await server.close();
    }
  });
}
