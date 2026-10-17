# Session prompts

Work streams from `docs/PLAN.md` §9, one prompt per session.

| Wave | Session | Start after |
|------|---------|-------------|
| 1 | [S1 Foundation](S1-foundation.md) | now; runs alone |
| 2 | [S2 Types + analysis](S2-analysis.md) | S1 merged |
| 2 | [S3 Vite plugin, npm, CLI distribution](S3-vite-npm-cli.md) | S1 merged |
| 2 | [S7 WASM runtime, ABI, glue, build driver](S7-wasm-runtime.md) | S1 merged |
| 3 | S4 JS foundation passes, S6 Rust backend | S2 merged (prompts written then) |
| 4 | S5 JS speed passes, S8 partitioner | S4 / S6 + S7 |
| 5 | S9 integration, examples, benchmarks | S8 |

Rules shared by every session are in [common.md](common.md); each prompt tells the session
to read it.
