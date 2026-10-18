// Node resolve hook for the vendored libraries' extensionless relative imports
// (`./result`, `..`), which their bundlers and test runners resolve: tries `.js`, then
// `/index.js`. Registered with `node --import ./register.mjs`.
import { existsSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

export async function resolve(specifier, context, next) {
  if (/^\.\.?(\/|$)/.test(specifier) && !/\.[cm]?js$/.test(specifier) && context.parentURL) {
    for (const suffix of ['.js', '/index.js']) {
      const url = new URL(specifier.replace(/\/$/, '') + suffix, context.parentURL);
      const file = fileURLToPath(url);
      if (existsSync(file) && statSync(file).isFile()) return next(url.href, context);
    }
  }
  return next(specifier, context);
}
