# AGENTS.md

Read [CLAUDE.md](CLAUDE.md) and [docs/sessions/common.md](docs/sessions/common.md) first; this file adds the rules for pull requests.

## Pull requests

- **Write the PR body with the `pr` skill** from Matt Pocock's skills (`mattpocock-skills:pr`), filling in the repository's pull request template where there is one.
- **Resolve merge conflicts before you hand a PR over.** Rebase onto `origin/main` and push; a PR that conflicts with `main` isn't finished.
- **Mark the PR ready for review as soon as the initial draft is finished.** Don't wait for CI to run, for reviews, or for the owner to do it. Keep fixing CI failures and review findings on the ready PR.
