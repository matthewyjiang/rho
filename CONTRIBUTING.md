# Contributing

Thanks for contributing. Keep changes small and focused on one concern.

## Setup

This repo expects Rust **1.92** (`mise.toml`). That file also enables [mr-boxington](https://mr-boxington.jdx.dev/) for mise 2026.9.2 or newer, so an activated `cargo` uses the shared build cache. Without mise, `cargo` is unchanged. The first wrapped build moves an existing `target/` into the mbx cache and leaves a symlink at `target/`.

From the repo root:

```bash
cargo build
cargo test
```

Local workflow, the PTY harness, and MSRV details live in the
[development docs](https://matthewyjiang.github.io/rho/development).

## Pull requests

- Use [Conventional Commits](https://www.conventionalcommits.org/) for the PR title and commits: `type(scope): description`. Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`. Description is imperative and lowercase, with no trailing period.
- You must follow the pull request template. Delete sections that do not apply.
- `!` and `BREAKING CHANGE:` publish a new major. Use them only after the maintainer approves a major release. Describe behavior changes that ship in a minor without them.
- If an AI made the change, end the body with one factual line naming the model and harness. If several models were used, name each one and what it did.
- Before a code PR, run `python3 scripts/validate.py full`. Title or body-only updates do not need a rebuild.
- Update docs for user-visible behavior.
- New or expanded tests follow `.agents/skills/rho-test-selection/SKILL.md`. Test a failure mode, not copy. Interactive TUI behavior uses the PTY harness.
- A worse API kept only for minor compatibility needs a `NEXT_MAJOR(...)` marker. See `.agents/skills/rho-next-major-debt/SKILL.md`.

## Code

- Prefer small modules and keep them private until something outside needs them.
- Match known enums exhaustively.

## License

Rho is MIT licensed. Contributions are under the same license.
