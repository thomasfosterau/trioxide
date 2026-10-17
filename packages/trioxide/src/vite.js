// The trioxide Vite plugin (Vite 8+). See vite.d.ts and README.md.

import { Project } from './index.js';
import { createPlugin } from './vite-plugin.js';

/**
 * @param {import('./vite.d.ts').TrioxidePluginOptions} [options]
 * @returns {import('vite').Plugin}
 */
export default function trioxide(options) {
  return createPlugin(options, { createProject: (root, projectOptions) => new Project(root, projectOptions) });
}
