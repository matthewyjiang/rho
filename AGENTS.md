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
- When the diff adds or materially expands tests, fill the test-gate section in the pull request template.

## Rust code

- Prefer small, cohesive modules with explicit public APIs. Keep modules private by default and export only the required crate surface.
- Avoid growing large files. Extract separable behavior into focused modules and keep tests and invariant documentation close to implementation.
- Make call sites self-documenting. Prefer enums, named methods, builders, or newtypes over ambiguous boolean or `Option` parameters. When an opaque positional boolean, `None`, or number is unavoidable, add an exact parameter-name comment, such as `set_mode(/*enabled*/ false)`.
- Match known enums exhaustively so new variants require intentional handling.
- Document new traits with their role and implementor expectations.
- For async traits, return an explicit future with a `Send` bound. Do not use `#[async_trait]` or `#[allow(async_fn_in_trait)]`.
- Avoid one-use helpers unless they materially improve readability or isolate a clear invariant.
- Follow Clippy and rustfmt style: collapse nested `if` statements when possible, inline format arguments (`format!("hello {name}")`), and prefer method references to redundant closures.
- After Rust changes, run the local checks that match CI quality gates when practical:
  - `cargo fmt --all` (CI enforces `cargo fmt --all -- --check`, including via `python3 scripts/check_sdk_compatibility.py --test-downstream`)
  - `python3 scripts/check_architecture.py`
  - the narrowest relevant tests
  - when touching `rho-sdk`, fixtures, or SDK packaging: `python3 scripts/check_sdk_compatibility.py --test-features` and `python3 scripts/check_sdk_compatibility.py --test-downstream`
  - before opening or updating a PR: `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` when the change is broad enough to warrant it
- Use the `rho-rust-change-validation` skill for the full workflow.

## Architecture and module boundaries

- Separate generic infrastructure from feature policy. Rendering, transport, storage, parsing, and orchestration should consume explicit generic data rather than know individual commands, menus, providers, or features.
- Keep feature-specific construction and decisions with the owning feature. For example, a picker renderer handles labels, details, badges, and selection state, while the model picker decides which model is selected.
- Model concepts such as selected, current, unavailable, warning, or detail explicitly instead of inferring them from encoded strings or suffixes.
- Split files that accumulate unrelated responsibilities along ownership boundaries: shared types and mechanics together, feature setup and policy in focused modules.
- If a file is subject to a custom legacy line budget, refactor cohesive behavior into appropriate modules to reduce the legacy file size. Do not satisfy the budget with formatting tricks, shortened names, compressed code, or other line-count workarounds.
- Design reusable components around stable concepts rather than current UI text or provider names, so new features provide data instead of adding component conditionals.
- Avoid broad abstractions before boundaries are clear. Once a pattern repeats, extract shared mechanics and leave differing policy at call sites.

## Rust tests

### Gate before you add a test

Every new test must earn its place. Prefer deleting or merging weak tests over growing the suite.

Before adding `#[test]` or `#[tokio::test]`, answer all three:

1. **Failure mode** - What user-visible or contract bug does this catch if it fails?
2. **Owner layer** - Which single layer owns that failure mode (see below)?
3. **Gap** - What existing test does *not* already cover it?

If you cannot name a distinct failure mode and one owner layer, do not add the test.

Put a one-line header on non-obvious new tests:

```rust
// Covers: cancel mid-tool must not commit partial results
// Owner: sdk orchestration
#[tokio::test]
async fn cancel_mid_tool_does_not_commit_partial_results() { ... }
```

### One failure mode, one owner layer

Pick the highest cheap layer that still fails for the right reason. Do not cover the same behavior at multiple layers.

| Owner layer | Use for | Home |
| --- | --- | --- |
| Pure unit | Parsers, policy tables, pure layout or wrap math, typed error mapping | sibling `*_tests.rs` next to the code |
| Runtime or SDK contract | Session lifecycle, cancel, retry, tool pairing, compaction, redaction | `rho-sdk` tests |
| Interactive UX | What the user sees after keys, paste, resize, submit, interrupt | PTY scenarios (`rho-tui-pty` / `tui_pty`) |
| OS or process | Path, shell, and FS behavior that differs by platform | narrow `#[cfg(...)]` tests |

Defaults:

- Interactive TUI behavior defaults to a **PTY scenario**, not a new unit test under `crates/rho/src/tui`.
- Add or extend a TUI unit test only for pure logic, or when a PTY scenario cannot express the failure mode cheaply. Say why in the PR.
- Do not add a unit render test, a layout buffer test, and a PTY scenario for the same change.

### Keep vs reject

**Prefer (Tier A)**

- Invariants that can regress silently: cancel, retry, resume, migration, replay, tool pairing
- Security and privacy: permissions, SSRF, credential storage, redaction
- Public SDK or provider wire contracts that break real backends
- PTY coverage for startup, submit, interrupt, pickers, resize, paste, shutdown
- Pure parsers and policy as **table-driven** unit tests

**Keep thin (Tier B)**

- Config migrations and invalid-config errors
- One structured test per rule, with cases in a table
- OS-specific path or shell edges

**Default reject (Tier C)**

- Happy-path tests whose only failure mode is "someone deleted the feature"
- Another case of the same branch with different literals as a new function
- Rendered chrome, labels, badges, help text, statusline copy, or theme strings
- `Default` / serde default checks that only restate derives
- Field-by-field load/save tests for each config key when one table or migration test would do
- Negative tests whose only purpose is to document removed behavior
- Static constants
- Private helper wiring already covered by a higher-layer test
- Duplicate coverage of the same branch at unit + integration + PTY

### How to write tests you do keep

- Prefer integration or behavior tests for user-visible logic and unit tests for focused pure logic.
- Put new test modules in sibling `*_tests.rs` files with explicit `#[path = "..."] mod tests;` declarations instead of growing implementation files.
- Prefer `pretty_assertions::assert_eq` when available and whole-object comparisons over field-by-field assertions.
- One test function per **rule**; put sibling inputs in a table. Do not add a new function for each literal unless it is a distinct rule.
- Assert decisions, enums, structured plans, and whole objects. Avoid `.contains("...")` on prose.
- String asserts are allowed only for redaction, wire-format contracts, and security escaping.
- Do not lock instructional prose, system-prompt wording, help text, or other copy behind string-contains tests. Review that text in the PR. Test assembly seams and user-visible behavior instead, such as conditional inclusion, tool gating, and end-to-end effects.
- Do not test static constants or add negative tests solely for removed behavior.
- Avoid mutating process environment; pass environment-derived values or dependencies explicitly when possible.
- When a PR adds tests, prefer removing or merging weaker tests in the same area so net suite weight stays flat or falls.

### Review bar for test-only growth

Reviewers should ask for changes when a PR:

- adds TUI unit tests for interactive behavior without a PTY scenario or a clear reason PTY is the wrong layer
- adds many near-duplicate test functions instead of a table
- locks copy or chrome strings
- adds Tier C tests without a distinct failure mode
- grows test LOC materially without deleting or tightening anything nearby

## Rho TUI testing

Prefer the deterministic PTY harness for automated interactive TUI regressions. PTY is the product gate for interactive behavior. Unit tests under `crates/rho/src/tui` stay limited to pure logic.

- harness crate: `crates/rho-tui-pty`
- smoke tests: `cargo test -p rho-coding-agent --test tui_pty`
- single scenario: `cargo run -p rho-tui-pty --bin rho-pty-scenario -- --bin target/debug/rho <scenario>`
- skill: `rho-tui-pty-testing`

For interactive bugfixes, add or extend a PTY scenario by default. Do not add a large buffer-string unit test for the same bug unless the logic is pure and better expressed as a table.

Use Herdr sibling-pane smoke tests only for exploratory validation, novel bugs not yet covered by a scenario, or real-terminal parity checks. Follow `rho-tui-herdr-testing` for that workflow.

## Rho experience tests

When operating as Rho rather than another agent such as Claude or Pi, report problems experienced with the agent harness so the Rho experience can be improved.
