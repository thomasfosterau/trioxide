# trioxide

A TypeScript-aware optimising compiler built on [oxc](https://oxc.rs). Per function,
trioxide either emits **optimised, readable JavaScript** or transpiles it to **Rust compiled
to WebAssembly**, with generated glue between the two. It ships as a standalone binary and as
a Vite plugin.

It is not a minifier: output keeps names, structure and comments. Types come from
[tsrs](https://github.com/oxc-project/tsrs), the Rust port of the TypeScript 7 checker, and
every fact trioxide relies on carries a trust level, because TypeScript's types are unsound.
Nothing that changes semantics happens on a fact that isn't proven.

See [`docs/PLAN.md`](docs/PLAN.md) for the design.

## Status

Early. Milestone M0 and the core of M1 are in place:

- the Cargo and pnpm workspaces, with oxc and tsrs building together;
- the interfaces every later stage plugs into (type oracle, fact tables, pass manager,
  partitioner, WASM island contract);
- a **passthrough pipeline**: module graph from entries (with tsconfig `paths`), parse,
  TypeScript strip honouring tsconfig `target`/`jsx`/class-field/decorator settings, and
  codegen with sourcemaps;
- the `trioxide` CLI (`transform`, `build [--watch]`, `check-toolchain`), a napi binding and
  the `trioxide` npm package with its Vite plugin (`trioxide/vite`);
- release builds of the binary and per-platform npm bindings;
- snapshot fixtures, a differential test suite and an example app compared with and without
  the plugin.

Not yet: type analysis, optimisation passes, WASM islands, `explain`.

## Installing

- **npm:** `npm i -D trioxide` gives you the `trioxide` command, the JS API and the Vite
  plugin; see [`packages/trioxide/README.md`](packages/trioxide/README.md). A prebuilt binding
  for your platform comes along as an optional dependency (`@trioxide/binding-<platform>`)
  for Linux x64/arm64 (glibc and musl), macOS x64/arm64 and Windows x64.
- **Standalone binary:** download `trioxide-<version>-<target>.tar.gz` (`.zip` on Windows)
  from the [GitHub releases](https://github.com/thomasfosterau/trioxide/releases) and check
  it against `SHA256SUMS`.
- **From source:** with Rust 1.99.1 or newer (via [rustup](https://rustup.rs)),

  ```sh
  cargo install --locked --git https://github.com/thomasfosterau/trioxide trioxide_cli
  # or, if your default toolchain is older:
  rustup toolchain install 1.99.1 && cargo +1.99.1 install --locked --git https://github.com/thomasfosterau/trioxide trioxide_cli
  ```

  installs `trioxide` into `~/.cargo/bin`. Add `--tag v<version>` for a release, or
  `--no-default-features` to build without the tsrs type checker (much faster to compile;
  types are then never more than `Declared`).

## Using it

```sh
trioxide transform src/main.ts                    # print one file as JavaScript
trioxide build src/main.ts --out-dir dist         # everything reachable, with sourcemaps
trioxide build src/main.ts --out-dir dist --watch # and again on every change
trioxide check-toolchain                          # can WASM islands be built here?
```

Options live in `trioxide.json` or `trioxide.jsonc`, found at or above the project root up to
the repository root (or pass `--config`); the schema is
[`schema/trioxide.schema.json`](schema/trioxide.schema.json). Diagnostics are coloured on a
terminal; `--color=never` or `NO_COLOR` turns that off.

In a Vite 8 app:

```ts
// vite.config.ts
import { defineConfig } from 'vite';
import trioxide from 'trioxide/vite';

export default defineConfig({ plugins: [trioxide()] });
```

From Node:

```js
import { transform, Project } from 'trioxide';

const { code, map } = transform('const x: number = 1;', 'src/x.ts');
```

## Building

Requirements: [rustup](https://rustup.rs), Node 22+ and pnpm 10. The Rust toolchain
(1.99.1 with the `wasm32-unknown-unknown` target) comes from `rust-toolchain.toml`; run
`rustup toolchain install` in the checkout if rustup doesn't install it on first use.

```sh
cargo build                     # every crate; the first build compiles tsrs and takes a few minutes
cargo test --workspace          # unit, snapshot and CLI tests
pnpm install
pnpm --filter trioxide build    # the napi binding (debug); build:release for an optimised one
pnpm -r test                    # npm package, plugin, dom-app and differential tests
```

The browser tests use Playwright's Chromium if it is installed, else `CHROMIUM_PATH` or a
system Chrome; they never download a browser.

Releases: pushing a `v*` tag runs `.github/workflows/release.yml`, which cross-builds the
binary and the binding for every platform, attaches the binaries and `SHA256SUMS` to a GitHub
release and publishes the npm packages (when the `NPM_TOKEN` secret is set). On pull requests
that touch the release path it runs as a dry run.

## Layout

| Path | What |
|------|------|
| `crates/trioxide` | Facade: `Options`, `Project`, outputs, the per-module pipeline |
| `crates/trioxide_common` | Ids, diagnostics and decision-log records shared by all crates |
| `crates/trioxide_project` | Module graph, resolution, tsconfig, content-hash cache |
| `crates/trioxide_types` | Type lattice, `TypeOracle`, and the tsrs adapter (only user of tsrs) |
| `crates/trioxide_analysis` | Fact tables with trust levels |
| `crates/trioxide_js` | Pass manager, optimisation passes, TS strip and codegen |
| `crates/trioxide_partition` | JS vs WASM decisions |
| `crates/trioxide_rust` | Rust backend (subset checker, RIR, emitter) |
| `crates/trioxide_wasm` | Island contract, cargo driver, glue |
| `crates/trioxide_rt` | Runtime for generated island crates (`no_std`, wasm32) |
| `crates/trioxide_cli` | The `trioxide` CLI (a library, so the npm bin runs the same code) |
| `crates/trioxide_napi` | Node-API binding |
| `packages/trioxide` | The npm package: JS API, `trioxide/vite`, bin, platform packages in `npm/` |
| `examples/dom-app` | A Vite + TS DOM app, built with and without trioxide and compared |
| `tests/fixtures` | Snapshot fixtures |
| `tests/differential` | Runs fixtures stripped-only vs through trioxide and compares output |

## License

MIT
