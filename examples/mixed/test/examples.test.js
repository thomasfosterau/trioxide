// Builds the examples with the Vite plugin, in production and in dev, and checks they compute
// what `trioxide build` (the CLI) builds: this app in Chromium (production with its island,
// dev without, as WASM is off in dev), and the CLI examples image-filters and telemetry as an
// SSR build and through the dev server's `ssrLoadModule`.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { build, createServer, preview } from 'vite';
import { afterAll, beforeAll, expect, test } from 'vitest';
import trioxide from 'trioxide/vite';

import { launchChromium } from '../../../packages/trioxide/test/browser.js';

const examples = fileURLToPath(new URL('../..', import.meta.url));
const repo = path.resolve(examples, '..');
const work = path.join(examples, 'mixed', 'node_modules', '.examples-test');

function cli() {
  const found = ['debug', 'release'].map((p) => path.join(repo, 'target', p, 'trioxide')).find((p) => existsSync(p));
  if (process.env.TRIOXIDE_BIN) return process.env.TRIOXIDE_BIN;
  if (found) return found;
  execFileSync('cargo', ['build', '--quiet', '-p', 'trioxide_cli'], { cwd: repo, stdio: 'inherit' });
  return path.join(repo, 'target', 'debug', 'trioxide');
}

/** `run()` of the example built by the CLI, its islands ready. */
async function cliRun(name) {
  const dir = path.join(work, 'cli', name);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  writeFileSync(path.join(dir, 'package.json'), '{ "type": "module" }\n');
  const root = path.join(examples, name);
  execFileSync(cli(), ['build', '--root', root, '--out-dir', dir, path.join(root, 'src/main.ts')], { stdio: 'pipe' });
  const module = await import(pathToFileURL(path.join(dir, 'src/main.js')).href);
  const islands = path.join(dir, '_islands');
  if (existsSync(islands)) {
    for (const glue of ['kernel.js', 'filters.js'].filter((file) => existsSync(path.join(islands, file)))) {
      await (await import(pathToFileURL(path.join(islands, glue)).href)).ready();
    }
  }
  return module.run();
}

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

test('mixed renders what the CLI build computes, built by Vite and served by its dev server', async () => {
  const root = path.join(examples, 'mixed');
  const expected = await cliRun('mixed');
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

for (const name of ['image-filters', 'telemetry']) {
  test(`${name} computes the same built by Vite (SSR) and through its dev server`, async () => {
    const root = path.join(examples, name);
    const expected = await cliRun(name);
    const config = { root, configFile: false, logLevel: 'error', plugins: [trioxide()] };
    const outDir = path.join(work, `${name}-dist`);
    await build({ ...config, build: { ssr: 'src/main.ts', outDir, emptyOutDir: true } });
    const built = await import(pathToFileURL(path.join(outDir, 'main.js')).href);
    expect(built.run()).toEqual(expected);
    const server = await createServer({ ...config, server: { middlewareMode: true } });
    try {
      expect((await server.ssrLoadModule('/src/main.ts')).run()).toEqual(expected);
    } finally {
      await server.close();
    }
  });
}
