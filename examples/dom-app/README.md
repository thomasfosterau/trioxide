# dom-app

A small Vite 8 + TypeScript DOM app (a task list) with nothing worth moving to WebAssembly. It
checks that the trioxide Vite plugin changes nothing observable in an ordinary app.

```sh
pnpm dev            # dev server with trioxide
pnpm build          # dist/ with trioxide
pnpm build:plain    # dist-plain/ with plain Vite
pnpm test           # builds both and compares a scripted session in Chromium
```

The test needs a Chromium: Playwright's managed browser if installed, else `CHROMIUM_PATH` or a
system Chrome (see `packages/trioxide/test/browser.js`).
