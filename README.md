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
- the `trioxide` CLI (`transform`, `build`), a napi binding and the `trioxide` npm package;
- snapshot fixtures and a differential test suite.

Not yet: type analysis, optimisation passes, WASM islands, `explain`, the Vite plugin.

## Using it

```sh
# Print one file as JavaScript.
cargo run -p trioxide_cli -- transform src/main.ts

# Compile everything reachable from the entries into dist/, with sourcemaps.
cargo run -p trioxide_cli -- build src/main.ts --out-dir dist
```

Options live in `trioxide.json` or `trioxide.jsonc` at the project root (or pass
`--config`); the schema is [`schema/trioxide.schema.json`](schema/trioxide.schema.json).

From Node, after building the package:

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
pnpm -r test                    # npm package tests and the differential suite
```

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
| `crates/trioxide_cli` | The `trioxide` binary |
| `crates/trioxide_napi` | Node-API binding |
| `packages/trioxide` | The npm package |
| `tests/fixtures` | Snapshot fixtures |
| `tests/differential` | Runs fixtures stripped-only vs through trioxide and compares output |

## License

MIT
