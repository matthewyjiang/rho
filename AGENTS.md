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
- When the diff adds or materially expands tests, follow the `rho-test-selection` skill and fill the test-gate section in the pull request template.

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

## Next-major cleanup markers

When a change ships a **minor-compatible compromise** that is deliberately worse
than the ideal API (semver field adds, dual variants, deprecated dual-emits,
temporary indirection), leave a greppable marker so the next major can clean it
up in one pass. The marker in code is the source of truth—not a separate
tracking issue alone.

### Marker line

Use this exact token so inventory is one search:

```text
NEXT_MAJOR(<crate-or-surface>): <imperative cleanup>
```

Examples:

```text
NEXT_MAJOR(rho-sdk): collapse RetryableFailure and RetryableFailureWithRetryAfter into one shape with optional retry_after
NEXT_MAJOR(rho-sdk): remove ProviderActivity and PROVIDER_ACTIVITY_* dual-emits
```

### Where to put it

1. **On the compromised API (required).**
   - Public items: a rustdoc `# Next major` section that includes the `NEXT_MAJOR(...)`
     line, the preferred end state, and why the compromise exists.
   - Private implementation: a `// NEXT_MAJOR(...): ...` comment at the site of the split.
2. **Host-facing docs (preferred when hosts match the awkward shape)** so external
   callers know the temporary contract and the intended collapse.
3. Prefer matching/construction helpers that cover every arm of the compromise, and
   document those helpers as the stable surface until major.

### What to mark

Mark when you chose a worse shape **only** to stay minor-compatible, including:

- Parallel enum variants that should be one variant (or event field) plus `Option`
- Metadata that belongs on an existing struct variant but could not be added without a major break
- Deprecated dual-emits or aliases retained solely for 1.x hosts
- Public helpers that exist only to paper over that temporary split

Do **not** use this for ordinary TODOs, nits, or cleanups that can land in a minor.

### Inventory and major cutover

```bash
rg 'NEXT_MAJOR\(' -n
```

When cutting a major release: run that search, land each cleanup, delete the
markers, and call out the breaks in release notes / the upgrade guide with a
`BREAKING CHANGE` section.

## Architecture and module boundaries

- Separate generic infrastructure from feature policy. Rendering, transport, storage, parsing, and orchestration should consume explicit generic data rather than know individual commands, menus, providers, or features.
- Keep feature-specific construction and decisions with the owning feature. For example, a picker renderer handles labels, details, badges, and selection state, while the model picker decides which model is selected.
- Model concepts such as selected, current, unavailable, warning, or detail explicitly instead of inferring them from encoded strings or suffixes.
- Split files that accumulate unrelated responsibilities along ownership boundaries: shared types and mechanics together, feature setup and policy in focused modules.
- If a file is subject to a custom legacy line budget, refactor cohesive behavior into appropriate modules to reduce the legacy file size. Do not satisfy the budget with formatting tricks, shortened names, compressed code, or other line-count workarounds.
- Design reusable components around stable concepts rather than current UI text or provider names, so new features provide data instead of adding component conditionals.
- Avoid broad abstractions before boundaries are clear. Once a pattern repeats, extract shared mechanics and leave differing policy at call sites.

## Rust tests

Use the `rho-test-selection` skill when adding, expanding, reviewing, or deleting tests. It is the source of truth for:

- failure-mode / owner-layer gate
- Tier A/B/C keep vs reject
- determinism (no sleep-sync or known flakes)
- table-driven style and assertion rules
- PTY as the interactive TUI product gate

Short defaults:

- Prefer behavior or integration tests for user-visible logic and unit tests for focused pure logic.
- Put new test modules in sibling `*_tests.rs` files with explicit `#[path = "..."] mod tests;`.
- Prefer `pretty_assertions::assert_eq` and whole-object comparisons when available.
- Do not test static constants or removed behavior; do not lock copy behind string-contains tests.
- Avoid mutating process environment; inject environment-derived values instead.
- Interactive TUI defaults to PTY (`rho-tui-pty-testing`); use Herdr only for exploration (`rho-tui-herdr-testing`).

## Rho experience tests

When operating as Rho rather than another agent such as Claude or Pi, report problems experienced with the agent harness so the Rho experience can be improved.
