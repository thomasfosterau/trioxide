import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { build, createServer } from 'vite';
import { afterAll, beforeAll, describe, test } from 'vitest';

import { Project, WASM_URL_PLACEHOLDER } from 'trioxide';
import trioxide from 'trioxide/vite';

import { collectEntries, createPlugin, resolveOptions, toRollupError } from '../src/vite-plugin.js';
import { launchChromium } from './browser.js';

/** A wasm module exporting `add(i32, i32) -> i32`. */
const WASM_ADD = Buffer.from([
  0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x60, 0x02, 0x7f, 0x7f, 0x01, 0x7f, 0x03,
  0x02, 0x01, 0x00, 0x07, 0x07, 0x01, 0x03, 0x61, 0x64, 0x64, 0x00, 0x00, 0x0a, 0x09, 0x01, 0x07, 0x00, 0x20,
  0x00, 0x20, 0x01, 0x6a, 0x0b,
]);

/** Glue in the shape S7 generates: it finds its `.wasm` through the URL placeholder. */
const FAKE_GLUE = `const url = new URL(${JSON.stringify(WASM_URL_PLACEHOLDER)}, import.meta.url);
const { instance } = await WebAssembly.instantiate(await (await fetch(url)).arrayBuffer());
export const add = instance.exports.add;
`;

/**
 * Wraps a real Project so its whole-program build contains a fake island `fake`, which main.ts
 * calls in place of `island()`.
 */
function withFakeIsland(root, options) {
  const project = new Project(root, options);
  return {
    get root() {
      return project.root;
    },
    build(entries) {
      const output = project.build(entries);
      for (const module of output.modules) {
        if (!module.path.endsWith('main.ts')) continue;
        module.code = `import { add } from "\\0trioxide:island/fake";\n${module.code.replace('island()', 'add(2, 3)')}`;
        module.map = undefined;
        module.islands = [{ id: 'fake', specifier: '\0trioxide:island/fake' }];
      }
      output.islands = [{ id: 'fake', wasm: WASM_ADD, glue: FAKE_GLUE }];
      return output;
    },
    update: (...args) => project.update(...args),
    transform: (...args) => project.transform(...args),
    explain: (...args) => project.explain(...args),
  };
}

function tempProject(files) {
  const root = mkdtempSync(path.join(tmpdir(), 'trioxide-vite-'));
  for (const [name, text] of Object.entries(files)) {
    mkdirSync(path.dirname(path.join(root, name)), { recursive: true });
    writeFileSync(path.join(root, name), text);
  }
  return root;
}

/** Builds `main.ts` as an ES module that keeps its exports, and imports the result. */
async function buildAndImport(root, plugin, extra = {}) {
  const outDir = path.join(root, 'dist');
  await build({
    root,
    configFile: false,
    logLevel: 'silent',
    plugins: [plugin],
    build: {
      outDir,
      target: 'esnext',
      minify: false,
      sourcemap: true,
      rolldownOptions: {
        input: path.join(root, 'main.ts'),
        preserveEntrySignatures: 'strict',
        output: { entryFileNames: '[name].js' },
      },
      ...extra,
    },
  });
  return { outDir, module: await import(`${pathToFileURL(path.join(outDir, 'main.js'))}?${Date.now()}`) };
}

// `(a < b) > (c, 0)` prints as `a < b > (c, 0)`, which TypeScript parses as the generic call
// `a<b>(c, 0)`: if Vite's oxc transform ran again on trioxide's output, `r` would throw.
const AMBIGUOUS = 'const a = 1, b = 2, c = 3;\nexport const r: boolean = (a < b) > (c, 0);\n';

describe('options', () => {
  test('trioxide.json is merged under the plugin options', () => {
    const root = tempProject({
      '.git/HEAD': '',
      'trioxide.jsonc': '{ // c\n "trustTypes": true, "wasm": { "wasmOpt": false }, "tsconfig": "cfg/tsconfig.json" }',
      'app/index.html': '',
    });
    const options = resolveOptions(
      { wasm: { glue: 'ready' }, check: true },
      { root: path.join(root, 'app'), command: 'build', build: {} },
    );
    assert.deepEqual(options, {
      trustTypes: true,
      check: true,
      wasm: { wasmOpt: false, glue: 'ready' },
      tsconfig: path.join(root, 'cfg/tsconfig.json'),
      world: 'app',
    });
    assert.deepEqual(resolveOptions({ configFile: false }, { root, command: 'build', build: {} }), { world: 'app' });
  });

  test('libraries are open world, and dev has no WASM unless asked', () => {
    const root = tempProject({ '.git/HEAD': '' });
    const lib = resolveOptions({}, { root, command: 'build', build: { lib: { entry: 'a.ts' } } });
    assert.equal(lib.world, 'library');
    assert.equal(resolveOptions({ world: 'app' }, { root, command: 'build', build: { lib: {} } }).world, 'app');
    assert.deepEqual(resolveOptions({}, { root, command: 'serve', build: {} }).wasm, { enabled: false });
    assert.equal(resolveOptions({ dev: { wasm: true } }, { root, command: 'serve', build: {} }).wasm, undefined);
  });

  test('a bad config file is reported', () => {
    const root = tempProject({ '.git/HEAD': '', 'trioxide.json': '{ "mdoe": "auto" }' });
    assert.throws(() => resolveOptions({}, { root, command: 'build', build: {} }), /unknown field `mdoe`/);
  });

  test('entries come from TS inputs and HTML module scripts', () => {
    const root = tempProject({
      'index.html': `<script type="module" src="/src/main.ts?x"></script>
        <script src="/classic.ts"></script>
        <script type='module' src='./other.mts'></script>
        <script type="module" src="https://cdn.example/x.ts"></script>
        <script type="module">inline()</script>`,
    });
    assert.deepEqual(collectEntries({ index: 'index.html', worker: 'src/worker.ts', css: 'a.css' }, root), [
      path.join(root, 'src/main.ts'),
      path.join(root, 'other.mts'),
      path.join(root, 'src/worker.ts'),
    ]);
  });
});

describe('build', () => {
  test('modules are compiled once, by trioxide, with sourcemaps', async () => {
    const root = tempProject({
      'main.ts': "import { r } from './ambiguous.js';\nexport function main(): string {\n  return `r=${r}`;\n}\n",
      'ambiguous.ts': AMBIGUOUS,
    });
    const { outDir, module } = await buildAndImport(root, trioxide({ configFile: false }));
    assert.equal(module.main(), 'r=true');
    const map = JSON.parse(readFileSync(path.join(outDir, 'main.js.map'), 'utf8'));
    assert.deepEqual(map.sources, ['../main.ts']);
    assert.match(map.sourcesContent[0], /main\(\): string/, 'maps back to the TypeScript');
  });

  test('island glue is served as a virtual module and its .wasm emitted as a hashed asset', async () => {
    const root = tempProject({
      'main.ts': 'declare function island(): number;\nexport function main(): number {\n  return island();\n}\n',
    });
    const plugin = createPlugin({ configFile: false }, { createProject: withFakeIsland });
    const fetchBefore = globalThis.fetch;
    globalThis.fetch = async (url) => new Response(readFileSync(new URL(url)));
    try {
      const { outDir, module } = await buildAndImport(root, plugin);
      assert.equal(module.main(), 5);
      const assets = readdirSync(path.join(outDir, 'assets'));
      const wasm = assets.find((name) => /^fake-[\w-]{8}\.wasm$/.test(name));
      assert.ok(wasm, assets.join());
      assert.deepEqual(readFileSync(path.join(outDir, 'assets', wasm)), WASM_ADD);
      const code = readFileSync(path.join(outDir, 'main.js'), 'utf8');
      assert.ok(code.includes(`new URL("./assets/${wasm}", `), code);
      assert.ok(!code.includes('__TRIOXIDE_WASM'), code);
    } finally {
      globalThis.fetch = fetchBefore;
    }
  });

  test('virtual island modules resolve, load and report unknown ids', () => {
    const plugin = createPlugin({ configFile: false }, { createProject: withFakeIsland });
    const root = tempProject({ 'main.ts': 'export const x = 1;\n' });
    plugin.configResolved({ root, command: 'serve', build: {} });
    assert.equal(plugin.resolveId('\0trioxide:island/fake'), '\0trioxide:island/fake');
    assert.equal(plugin.resolveId('trioxide:island/fake'), '\0trioxide:island/fake');
    assert.equal(plugin.resolveId('./main.ts'), undefined);
    const context = { error: (message) => assert.fail(message), warn() {} };
    // Dev with dev.wasm: the whole program is built at start and the wasm inlined.
    const devPlugin = createPlugin({ configFile: false, dev: { wasm: true } }, { createProject: withFakeIsland });
    devPlugin.configResolved({ root, command: 'serve', build: {} });
    devPlugin.buildStart.call(context, { input: path.join(root, 'main.ts') });
    const glue = devPlugin.load.call(context, '\0trioxide:island/fake');
    const dataUrl = `data:application/wasm;base64,${WASM_ADD.toString('base64')}`;
    assert.ok(glue.code.includes(`new URL("${dataUrl}", import.meta.url)`), glue.code);
    assert.equal(devPlugin.load.call(context, '/main.ts'), undefined);
    assert.throws(
      () =>
        plugin.load.call(
          {
            error(message) {
              throw new Error(message);
            },
          },
          '\0trioxide:island/nope',
        ),
      /unknown island "nope"/,
    );
  });

  test('errors carry a location and code frame', async () => {
    const root = tempProject({ 'main.ts': "// ünïcödé\nexport const x: number = 1;\nlet y: = 2;\n" });
    await assert.rejects(buildAndImport(root, trioxide({ configFile: false })), (error) => {
      assert.match(error.message, /Unexpected token/);
      return true;
    });
    const error = toRollupError(
      {
        severity: 'error',
        code: 'parse',
        message: 'Unexpected token',
        file: path.join(root, 'main.ts'),
        labels: [{ start: Buffer.byteLength('// ünïcödé\nexport const x: number = 1;\nlet y: '), end: 0 }],
        help: 'Try removing it',
      },
      readFileSync(path.join(root, 'main.ts'), 'utf8'),
    );
    assert.equal(error.message, '[parse] Unexpected token\nhelp: Try removing it');
    assert.deepEqual(error.loc, { file: path.join(root, 'main.ts'), line: 3, column: 7 });
    assert.equal(
      error.frame,
      [
        '  1 | // ünïcödé',
        '  2 | export const x: number = 1;',
        '> 3 | let y: = 2;',
        '    |        ^',
        '  4 | ',
      ].join('\n'),
    );
  });
});

describe('dev server', () => {
  test('dependants whose output changes are invalidated with the changed module', async () => {
    const root = tempProject({
      'main.ts': "import { label } from './label.js';\nexport const text: string = label;\n",
      'label.ts': "export const label: string = 'one';\n",
    });
    // Pretend cross-module inlining: main.ts's output depends on label.ts's source.
    let labelVersion = 0;
    let inline = false;
    const createProject = (projectRoot, options) => {
      const project = new Project(projectRoot, options);
      return {
        update(file, source) {
          if (file.endsWith('label.ts')) labelVersion += 1;
          return project.update(file, source);
        },
        transform(file) {
          const out = project.transform(file);
          if (inline && file.endsWith('main.ts')) out.code += `// label v${labelVersion}\n`;
          return out;
        },
      };
    };
    const plugin = createPlugin({ configFile: false }, { createProject });
    plugin.configResolved({ root, command: 'serve', build: {} });
    const context = { error: (error) => assert.fail(error.message ?? error), warn() {} };
    for (const name of ['main.ts', 'label.ts']) {
      plugin.transform.call(context, readFileSync(path.join(root, name), 'utf8'), path.join(root, name));
    }
    const invalidated = [];
    const environment = {
      moduleGraph: {
        getModulesByFile: (file) => new Set([{ file }]),
        invalidateModule: (module) => invalidated.push(module.file),
      },
    };
    const label = { file: path.join(root, 'label.ts') };
    const update = (source) =>
      plugin.hotUpdate.call(
        { environment },
        { file: label.file, type: 'update', read: () => source, modules: [label], timestamp: 0 },
      );

    // main.ts's output is unchanged: Vite's normal HMR propagation applies.
    assert.equal(await update("export const label: string = 'two';\n"), undefined);
    assert.deepEqual(invalidated, []);

    inline = true;
    const modules = await update("export const label: string = 'three';\n");
    assert.deepEqual(
      modules.map((module) => path.basename(module.file)),
      ['label.ts', 'main.ts'],
    );
    assert.deepEqual(invalidated.map((file) => path.basename(file)), ['main.ts']);
  });

  describe('in Chromium', () => {
    let browser;
    beforeAll(async () => {
      browser = await launchChromium();
    });
    afterAll(async () => {
      await browser?.close();
    });

    // Vite's watcher drops a second change to a file within ~50 ms of the first, so edits are
    // spaced out the way a person's would be.
    async function edit(file, text) {
      await new Promise((resolve) => setTimeout(resolve, 200));
      writeFileSync(file, text);
    }

    async function serve(root, plugin) {
      const server = await createServer({
        root,
        configFile: false,
        logLevel: 'silent',
        plugins: [plugin],
        server: { host: '127.0.0.1', port: 0 },
      });
      await server.listen();
      return server;
    }

    test('HMR updates modules in place and shows errors in the overlay', async () => {
      const root = tempProject({
        'index.html': '<!doctype html><div id="app"></div><script type="module" src="/main.ts"></script>\n',
        'main.ts': [
          "import { label } from './label.js';",
          "import { r } from './ambiguous.js';",
          "const app = document.querySelector<HTMLDivElement>('#app')!;",
          'app.textContent = `${label}:${r}`;',
          'if (import.meta.hot) import.meta.hot.accept();',
          '',
        ].join('\n'),
        'label.ts': "export const label: string = 'one';\n",
        'ambiguous.ts': AMBIGUOUS,
      });
      const server = await serve(root, trioxide({ configFile: false }));
      const page = await browser.newPage();
      try {
        await page.goto(server.resolvedUrls.local[0]);
        await page.waitForFunction(() => document.querySelector('#app')?.textContent === 'one:true');
        await page.evaluate(() => {
          window.notReloaded = true;
        });

        await edit(path.join(root, 'label.ts'), "export const label: string = 'two';\n");
        await page.waitForFunction(() => document.querySelector('#app')?.textContent === 'two:true');
        assert.equal(await page.evaluate(() => window.notReloaded), true, 'updated without a full reload');

        await edit(path.join(root, 'label.ts'), "export const label: string = ;\n");
        const overlay = await page.waitForSelector('vite-error-overlay', { state: 'attached', timeout: 15_000 });
        const text = await overlay.evaluate((element) => element.shadowRoot.textContent);
        assert.match(text, /\[plugin:trioxide\] Unexpected token/);
        assert.match(text, /label\.ts:1:29/);
        assert.match(text, /> 1 \| export const label: string = ;/);

        await edit(path.join(root, 'label.ts'), "export const label: string = 'three';\n");
        await page.waitForFunction(() => document.querySelector('#app')?.textContent === 'three:true');
        assert.equal(await page.$('vite-error-overlay'), null);
      } finally {
        await page.close();
        await server.close();
      }
    });

    test('dev.wasm serves island glue with the .wasm inlined', async () => {
      const root = tempProject({
        'index.html': '<!doctype html><div id="app"></div><script type="module" src="/main.ts"></script>\n',
        'main.ts': "declare function island(): number;\ndocument.querySelector('#app')!.textContent = String(island());\n",
      });
      const plugin = createPlugin({ configFile: false, dev: { wasm: true } }, { createProject: withFakeIsland });
      const server = await serve(root, plugin);
      const page = await browser.newPage();
      try {
        await page.goto(server.resolvedUrls.local[0]);
        await page.waitForFunction(() => document.querySelector('#app')?.textContent === '5');
      } finally {
        await page.close();
        await server.close();
      }
    });
  });
});
