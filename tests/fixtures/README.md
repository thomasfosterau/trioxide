# Fixtures

Each directory with an `input.ts` or `input.tsx` is a fixture. It may contain other modules
(imported with `.js` specifiers, the TypeScript ESM convention), its own `tsconfig.json`
(otherwise `tests/fixtures/tsconfig.json` applies) and a `trioxide.json`.

- **Snapshots** (`cargo test -p trioxide --test fixtures`): every module of the build is
  printed into `output.snap` next to the input. Review changes with `cargo insta review`.
- **Differential** (`pnpm --filter trioxide-differential test`): if the entry exports
  `main()`, it runs three ways in Node (types stripped only, `trioxide --emit=js`, and later
  JS+WASM) and the stdout must be identical. Keep `main()` deterministic.

Directories can nest (e.g. `analysis/<case>/`).
