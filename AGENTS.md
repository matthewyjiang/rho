# AGENTS.md

## Commits and pull requests

Use Conventional Commits for commit messages and PR titles:

```text
<type>(<scope>): <description>
```

- Allowed types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`.
- Scope is optional but preferred when useful.
- Use a concise, imperative, lowercase description unless it contains a proper noun; do not end it with a period.
- For breaking changes, add `!` after the type or scope and a `BREAKING CHANGE:` footer.

Examples:

```text
feat(auth): add token refresh
fix(api): handle empty responses
docs: update setup instructions
chore: bump dependencies
feat(config)!: require explicit config path

BREAKING CHANGE: the default config discovery behavior was removed.
```

For PRs:

- Prefer the most user-visible type, usually `feat`, `fix`, `docs`, or `refactor`.
- Clearly summarize what changed and why, list validation, and call out breaking changes with a `BREAKING CHANGE:` section.
- Update documentation for important user-visible changes.

## Rust code

- Prefer small, cohesive modules with explicit public APIs. Keep modules private by default and export only the required crate surface.
- Avoid growing large files. Extract separable behavior into focused modules and keep tests and invariant documentation close to implementation.
- Make call sites self-documenting. Prefer enums, named methods, builders, or newtypes over ambiguous boolean or `Option` parameters. When an opaque positional boolean, `None`, or number is unavoidable, add an exact parameter-name comment, such as `set_mode(/*enabled*/ false)`.
- Match known enums exhaustively so new variants require intentional handling.
- Document new traits with their role and implementor expectations.
- For async traits, return an explicit future with a `Send` bound. Do not use `#[async_trait]` or `#[allow(async_fn_in_trait)]`.
- Avoid one-use helpers unless they materially improve readability or isolate a clear invariant.
- Follow Clippy and rustfmt style: collapse nested `if` statements when possible, inline format arguments (`format!("hello {name}")`), and prefer method references to redundant closures.
- After Rust changes, run `cargo fmt`, `python3 scripts/check_architecture.py`, and the narrowest relevant tests when practical. Use the `rho-rust-change-validation` skill for the full workflow.

## Architecture and module boundaries

- Separate generic infrastructure from feature policy. Rendering, transport, storage, parsing, and orchestration should consume explicit generic data rather than know individual commands, menus, providers, or features.
- Keep feature-specific construction and decisions with the owning feature. For example, a picker renderer handles labels, details, badges, and selection state, while the model picker decides which model is selected.
- Model concepts such as selected, current, unavailable, warning, or detail explicitly instead of inferring them from encoded strings or suffixes.
- Split files that accumulate unrelated responsibilities along ownership boundaries: shared types and mechanics together, feature setup and policy in focused modules.
- If a file is subject to a custom legacy line budget, refactor cohesive behavior into appropriate modules to reduce the legacy file size. Do not satisfy the budget with formatting tricks, shortened names, compressed code, or other line-count workarounds.
- Design reusable components around stable concepts rather than current UI text or provider names, so new features provide data instead of adding component conditionals.
- Avoid broad abstractions before boundaries are clear. Once a pattern repeats, extract shared mechanics and leave differing policy at call sites.

## Rust tests

- Prefer integration or behavior tests for user-visible logic and unit tests for focused pure logic.
- Put new test modules in sibling `*_tests.rs` files with explicit `#[path = "..."] mod tests;` declarations instead of growing implementation files.
- Prefer `pretty_assertions::assert_eq` when available and whole-object comparisons over field-by-field assertions.
- Do not test static constants or add negative tests solely for removed behavior.
- Avoid mutating process environment; pass environment-derived values or dependencies explicitly when possible.

## Rho TUI testing

Prefer the deterministic PTY harness for automated interactive TUI regressions:

- harness crate: `crates/rho-tui-pty`
- smoke tests: `cargo test -p rho-coding-agent --test tui_pty`
- single scenario: `cargo run -p rho-tui-pty --bin rho-pty-scenario -- --bin target/debug/rho <scenario>`
- skill: `rho-tui-pty-testing`

Use Herdr sibling-pane smoke tests only for exploratory validation, novel bugs not yet covered by a scenario, or real-terminal parity checks. Follow `rho-tui-herdr-testing` for that workflow.

## Rho experience tests

When operating as Rho rather than another agent such as Claude or Pi, report problems experienced with the agent harness so the Rho experience can be improved.
