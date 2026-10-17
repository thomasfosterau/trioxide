// Builds the app with and without the trioxide plugin and checks that both behave the same in
// Chromium: the same DOM after every step of a scripted session, and the same console output.

import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';

import { build, preview } from 'vite';
import { afterAll, beforeAll, expect, test } from 'vitest';

import { launchChromium } from '../../../packages/trioxide/test/browser.js';

const root = fileURLToPath(new URL('..', import.meta.url));

/** Builds with vite.config.ts in `mode`; reports whether the trioxide plugin was active. */
async function buildApp(mode, outDir) {
  const seen = { plugin: false, oxcExcludesTs: false, project: false };
  let trioxidePlugin;
  const probe = {
    name: 'probe',
    configResolved(config) {
      trioxidePlugin = config.plugins.find((plugin) => plugin.name === 'trioxide');
      seen.plugin = Boolean(trioxidePlugin);
      seen.oxcExcludesTs = [config.oxc?.exclude].flat().some((pattern) => pattern instanceof RegExp && pattern.test('a.ts'));
    },
    buildEnd() {
      seen.project = Boolean(trioxidePlugin?.api.project);
    },
  };
  await build({ root, mode, logLevel: 'warn', plugins: [probe], build: { outDir, emptyOutDir: true } });
  return seen;
}

async function serve(mode, outDir) {
  return preview({ root, mode, logLevel: 'silent', build: { outDir }, preview: { host: '127.0.0.1', port: 0 } });
}

/** Drives the app and records what it shows after each step. */
async function session(browser, url) {
  const page = await browser.newPage();
  const log = [];
  page.on('console', (message) => log.push(`${message.type()}: ${message.text()}`));
  page.on('pageerror', (error) => log.push(`pageerror: ${error.message}`));
  const steps = [];
  const snapshot = async (step) => {
    steps.push({
      step,
      tasks: await page.$$eval('#tasks li', (items) =>
        items.map((item) => ({
          title: item.querySelector('span').textContent,
          priority: item.dataset.priority,
          done: item.classList.contains('done'),
          checked: item.querySelector('input').checked,
        })),
      ),
      summary: await page.textContent('#summary'),
      filters: await page.$$eval('#filters button', (buttons) => buttons.map((b) => `${b.textContent}:${b.getAttribute('aria-pressed')}`)),
    });
  };
  const add = async (title, priority) => {
    await page.fill('#title', title);
    await page.selectOption('#priority', priority);
    await page.click('#new-task button[type=submit]');
    await snapshot(`add ${JSON.stringify(title)} ${priority}`);
  };
  const task = (title) => page.locator('#tasks li', { hasText: title });

  await page.goto(url);
  await page.waitForSelector('#filters button');
  await snapshot('load');
  await add('Write docs', 'normal');
  await add('Fix bug', 'high');
  await add('Water plants', 'low');
  await add('   ', 'high');
  await task('Fix bug').locator('input').check();
  await snapshot('toggle Fix bug');
  for (const filter of ['Active', 'Done', 'All']) {
    await page.click(`#filters button:text("${filter}")`);
    await snapshot(`filter ${filter}`);
  }
  await task('Water plants').locator('button').click();
  await snapshot('remove Water plants');
  await task('Fix bug').locator('input').uncheck();
  await snapshot('untoggle Fix bug');
  await page.close();
  return { steps, log };
}

let browser;
const builds = {};

beforeAll(async () => {
  browser = await launchChromium();
  builds.trioxide = await buildApp('production', 'dist');
  builds.plain = await buildApp('plain', 'dist-plain');
});

afterAll(async () => {
  await browser?.close();
});

test('the trioxide build actually used the plugin', () => {
  assert.deepEqual(builds.trioxide, { plugin: true, oxcExcludesTs: true, project: true });
  assert.deepEqual(builds.plain, { plugin: false, oxcExcludesTs: false, project: false });
});

test('the app behaves the same with and without trioxide', async () => {
  const results = {};
  for (const [mode, outDir, name] of [
    ['production', 'dist', 'trioxide'],
    ['plain', 'dist-plain', 'plain'],
  ]) {
    const server = await serve(mode, outDir);
    try {
      results[name] = await session(browser, server.resolvedUrls.local[0]);
    } finally {
      await new Promise((resolve) => server.httpServer.close(resolve));
    }
  }

  assert.deepEqual(results.trioxide, results.plain);
  // And the session did what it should, so equality is not vacuous.
  const last = results.plain.steps.at(-1);
  expect(last.summary).toBe('2 tasks left of 2 (1 normal, 1 high).');
  expect(last.tasks.map((t) => t.title)).toEqual(['Fix bug', 'Write docs']);
  expect(results.plain.log).toEqual(['log: added #1: Write docs', 'log: added #2: Fix bug', 'log: added #3: Water plants']);
  expect(results.plain.steps.find((s) => s.step === 'filter Done').tasks).toEqual([
    { title: 'Fix bug', priority: 'high', done: true, checked: true },
  ]);
});
