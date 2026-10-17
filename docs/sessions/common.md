# Rules for every trioxide session

- `docs/PLAN.md` is the spec. Read it fully before starting. If you must deviate (an oxc or
  tsrs API doesn't allow something, or a better design emerges), make the change, update
  `docs/PLAN.md` in the same PR, and say why in the PR description.
- **Stay in your lane.** Your prompt lists the crates and packages you own. You may *read*
  everything. If you need a change to another stream's public interface, keep it minimal
  and additive, and call it out under a "Cross-stream changes" heading in the PR
  description. Never rework another stream's internals.
- Pins: all `oxc_*` crates `=0.154.0`; tsrs is a git dependency pinned to a commit; Rust
  1.99.1 via `rust-toolchain.toml`. Don't bump any of these unless your prompt says so.
- Before every push, run and pass: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
  `pnpm -r test` (once packages exist), and the differential suite once it exists.
- Tests come with the code: `insta` snapshots for transforms and analysis output, plus a
  differential fixture (original vs output, run in Node, identical stdout) for anything
  that changes runtime behaviour.
- Correctness beats coverage. When an optimisation or lowering can't be proven safe, skip
  it and record the reason (the decision log behind `trioxide explain`). Never guess.
- Match the surrounding code style. Public items get doc comments; keep internal comments
  sparse.
- Work on the branch your session was given, commit in logical steps, and open a PR to
  `main` when the acceptance criteria hold. The PR description lists what was done, what
  was deferred, and any cross-stream changes.
