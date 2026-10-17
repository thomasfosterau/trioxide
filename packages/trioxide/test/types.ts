// Compiled by `tsc -p test` (no emit): checks the published declarations from a consumer's
// point of view.

import { defineConfig } from 'vite';

import { Project, build, findConfig, loadConfig, transform, type Mode, type TrioxideOptions } from 'trioxide';
import trioxide, { type TrioxidePluginOptions } from 'trioxide/vite';

const options: TrioxideOptions = {
  mode: 'auto',
  world: 'app',
  wasm: { glue: 'ready' },
  overrides: [{ files: ['src/dom/**'], target: 'js' }],
};
// @ts-expect-error: not a mode
const badMode: Mode = 'fast';

const pluginOptions: TrioxidePluginOptions = { ...options, configFile: false, dev: { wasm: true } };
export default defineConfig({ plugins: [trioxide(pluginOptions)] });

const out = transform('let x: number = 1', '/a.ts', options);
const code: string = out.code;
const project = new Project('.', options);
const modules: number = build(['a.ts']).modules.length;
const config: string | null = findConfig('.');
const loaded: TrioxideOptions = loadConfig('trioxide.json');
const plugin = trioxide();
const name: string = plugin.name;
const projectOrUndefined: Project | undefined = plugin.api.project;
void [badMode, code, project, modules, config, loaded, name, projectOrUndefined];
