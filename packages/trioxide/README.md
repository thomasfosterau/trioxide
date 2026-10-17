# trioxide

A TypeScript-aware optimising compiler built on [oxc](https://oxc.rs): per function, readable
optimised JavaScript or Rust compiled to WebAssembly. This package is the JavaScript API, the
Vite plugin and the `trioxide` command. See the
[repository](https://github.com/thomasfosterau/trioxide) for the design and status.

```sh
npm i -D trioxide
```

Requires Node 22+. The compiler is native code: npm installs the prebuilt binding for your
platform as an optional dependency, one of

| Package | Platform |
|---------|----------|
| `@trioxide/binding-linux-x64-gnu` | Linux x64, glibc |
| `@trioxide/binding-linux-arm64-gnu` | Linux arm64, glibc |
| `@trioxide/binding-linux-x64-musl` | Linux x64, musl (Alpine) |
| `@trioxide/binding-linux-arm64-musl` | Linux arm64, musl |
| `@trioxide/binding-darwin-x64` | macOS Intel |
| `@trioxide/binding-darwin-arm64` | macOS Apple silicon |
| `@trioxide/binding-win32-x64-msvc` | Windows x64 |

If none was installed (e.g. `--omit=optional`), loading `trioxide` fails with the list of
what it tried. WebAssembly islands additionally need a Rust toolchain at build time; run
`npx trioxide check-toolchain`. Without one, every function stays JavaScript.

## Vite plugin

```ts
// vite.config.ts
import { defineConfig } from 'vite';
import trioxide from 'trioxide/vite';

export default defineConfig({
  plugins: [trioxide()],
});
```

Vite 8 or newer. The plugin:

- compiles `.ts`, `.tsx`, `.mts` and `.cts` modules itself, before any other plugin
  (`enforce: 'pre'`), and keeps Vite's own oxc transform off them so they are not compiled
  twice;
- reads `trioxide.json` / `trioxide.jsonc` (the nearest one at or above the Vite root, up to
  the repository root) and merges the plugin options over it;
- defaults `world` to `"app"` (the whole program is visible), or `"library"` with `build.lib`;
- on `vite build`, runs the whole-program analysis at build start from Rollup's inputs
  (TypeScript entries, and the module scripts of HTML entries), then serves each module's
  compiled output with its sourcemap;
- serves WebAssembly island glue as the virtual modules `\0trioxide:island/<id>` and emits
  each island's `.wasm` as a hashed asset (`assets/<id>-<hash>.wasm`), rewriting the glue to
  point at it;
- in the dev server, compiles modules on demand and keeps them up to date on change. A change
  that alters the output of modules importing the changed one (e.g. an inlined constant)
  invalidates those modules too; otherwise HMR works exactly as without the plugin. Islands
  are off in dev unless `dev: { wasm: true }`, in which case their `.wasm` is inlined;
- reports problems through Vite: errors stop the build or show in the dev server's error
  overlay with a code frame, warnings go to the terminal.

### Options

Everything in [`trioxide.json`](./schema.json) (typed as `TrioxideOptions`), plus:

| Option | Default | |
|--------|---------|---|
| `configFile` | discovered | Path of the `trioxide.json` to merge over (relative to the Vite root), or `false` to ignore config files |

```ts
trioxide({
  mode: 'auto', // 'js-only' | 'auto' | 'wasm-preferred'
  trustTypes: false, // treat declared TS types as proof
  dev: { wasm: true }, // build islands in the dev server too
  wasm: { require: true }, // fail instead of falling back to JS without a Rust toolchain
});
```

Other plugins can reach the plugin's state through `plugin.api` (`project`, `options`).

Known limitation: TSX modules get no React Fast Refresh transform, because Vite's oxc
transform (which adds it) no longer runs on them.

## JavaScript API

```js
import { Project, build, findConfig, loadConfig, transform } from 'trioxide';

// One module, from source.
const { code, map, diagnostics } = transform('const x: number = 1;', 'src/x.ts');

// The whole program.
const project = new Project('.', { world: 'app' });
const output = project.build(['src/main.ts']); // { modules, islands, report }
project.update('src/util.ts', newSource); // { changed, dependants }
project.transform('src/util.ts'); // the module's current output
```

Types are included (`TrioxideOptions` is generated from the Rust options schema). The schema
itself is exported as `trioxide/schema.json`, so config files can point at it:

```json
{ "$schema": "./node_modules/trioxide/schema.json", "mode": "auto" }
```

## Command line

`npx trioxide` runs the same CLI as the standalone binary (it is the Rust CLI, called through
the binding):

```sh
npx trioxide transform src/main.ts
npx trioxide build src/main.ts --out-dir dist --watch
npx trioxide check-toolchain
```

## Using a local checkout in a fresh Vite app

To try the plugin from a clone of the repository, before or instead of a published release:

1. Build the binding in the clone (needs Rust, Node 22+ and pnpm 10):

   ```sh
   git clone https://github.com/thomasfosterau/trioxide && cd trioxide
   pnpm install
   pnpm --filter trioxide build:release   # or `build` for a quicker debug build
   ```

   This writes `packages/trioxide/binding/trioxide.<platform>.node`, which the package loads
   before looking for an `@trioxide/binding-*` package.

2. Create a Vite 8 app next to it and depend on the package with a `file:` specifier:

   ```sh
   npm create vite@latest my-app -- --template vanilla-ts
   cd my-app
   npm install -D ../trioxide/packages/trioxide
   ```

   This records `"trioxide": "file:../trioxide/packages/trioxide"` and links the directory, so
   rebuilding the binding in the clone takes effect without reinstalling.

3. Create `vite.config.ts` with the plugin:

   ```ts
   // vite.config.ts
   import { defineConfig } from 'vite';
   import trioxide from 'trioxide/vite';

   export default defineConfig({ plugins: [trioxide()] });
   ```

4. `npm run dev` and `npm run build` now compile the app's TypeScript with trioxide.

`examples/dom-app` in the repository is such an app, wired up as a workspace package; its test
builds it with and without the plugin and compares the two in Chromium.

## Releases

Each `v*` tag of the repository publishes `trioxide` and the seven binding packages at the same
version, with npm provenance; the standalone binaries are attached to the GitHub release.
