---
name: rho-rust-change-validation
description: Validate and finalize Rust changes in Rho with the staged fast and full workflows. Use after editing Rust, before committing or opening a PR, when fixing test or lint failures, or when reviewing compliance with Rho's architecture and test conventions. Review the diff, run the narrowest useful checks during iteration, run comprehensive checks at the final gate, and report exactly what was validated.
compatibility: Requires a Rho checkout with Rust, Cargo, and Python 3.
---

# Validate Rho Rust changes

## 1. Establish scope

From the repository root, inspect staged and unstaged changes:

```bash
git status --short
git diff --stat
git diff -- src Cargo.toml Cargo.lock build.rs
git diff --cached --stat
```

Identify changed behaviors and modules, unrelated user work to preserve, the narrowest relevant tests, TUI smoke-test needs, and user-facing documentation impact.

## 2. Review the change

Correct clear in-scope issues before validation.

### Structure and APIs

- Keep modules cohesive, private by default, and explicit about their public API.
- Avoid growing large files when a focused module has a clear owner.
- Keep generic infrastructure separate from feature policy and put decisions near the owning feature.
- Model state explicitly instead of encoding concepts in display strings.
- Extract repeated mechanics, not speculative abstractions.
- Avoid opaque boolean, `Option`, and numeric arguments. Prefer enums, named methods, builders, or newtypes; otherwise add an exact parameter-name comment.
- Prefer exhaustive matches for known enums.
- Document new traits. For async traits, return an explicit `Send` future rather than using `async_trait` or allowing `async_fn_in_trait`.
- Avoid one-use helpers unless they clarify an invariant or materially improve readability.

### Tests

Load the `rho-test-selection` skill before adding, expanding, reviewing, or deleting tests. Enforce its failure-mode / owner-layer gate, Tier A/B/C rules, determinism rules, and PTY defaults.

Short checks while reviewing the diff:

- Prefer behavior or integration tests for user-visible behavior and unit tests for focused pure logic.
- Put new test modules in sibling `*_tests.rs` files with an explicit `#[path = "..."] mod tests;` when practical.
- Prefer `pretty_assertions::assert_eq` and whole-object comparisons when available.
- Do not test static constants or removed behavior; do not lock copy behind string-contains tests.
- Avoid mutating process environment; inject environment-derived values or dependencies instead.
- Reject sleep-synced or known-flaky tests; wait on explicit signals or use PTY harness waits.
- Interactive TUI changes default to PTY scenarios (`rho-tui-pty-testing`), not new chrome unit tests.

### Next-major debt

Load the `rho-next-major-debt` skill on every issue implementation and PR that
touches public contracts, events, errors, or any shape chosen only to stay
minor-compatible. Before finishing:

- If the ideal API was blocked by minor semver, confirm a greppable
  `NEXT_MAJOR(<surface>): <cleanup>` marker sits on the compromised API.
- Prefer helpers that cover every arm until major; update host docs when callers
  must match carefully.
- Do not leave dual variants, dual-emits, or temporary splits unmarked.

For bug fixes, reproduce the issue through the closest practical user path before finalizing when feasible.

## 3. Run validation

Use the repository validation wrapper instead of assembling overlapping Cargo commands by hand. It caps Cargo at 12 jobs, keeps the edit loop narrow, and reserves all-target and all-feature checks for the final gate.

Capture verbose output in temporary logs and inspect only relevant excerpts.

### Fast edit loop

After Rust changes, format first, then run the fast workflow for the owning package:

```bash
cargo fmt --all

VALIDATION_LOG=$(mktemp /tmp/rho-validation.XXXXXX.log)
python3 scripts/validate.py fast --package <package> >"$VALIDATION_LOG" 2>&1
```

The fast workflow checks formatting and architecture, then checks the selected package without compiling every target. Add the narrowest relevant test selection when behavior changed:

```bash
# Focused library or unit test
python3 scripts/validate.py fast \
  --package rho-sdk \
  --lib \
  --filter <test-name>

# Focused integration test
python3 scripts/validate.py fast \
  --package rho-coding-agent \
  --test <integration-target> \
  --filter <test-name>
```

Use the Cargo package name, such as `rho-coding-agent`, `rho-providers`, `rho-sdk`, `rho-agent-tools`, or `rho-tui-pty`. Prefer an explicit `--lib` or `--test` target with a filter so Cargo does not compile unrelated test binaries. Run additional behavior or integration targets only when the change crosses their boundaries.

If an SDK, downstream fixture, or SDK packaging change needs compatibility coverage before the full gate, run the matching focused command:

```bash
python3 scripts/check_sdk_compatibility.py --test-features
python3 scripts/check_sdk_compatibility.py --test-downstream
```

### Full final gate

Before opening or updating a PR, and after broad or cross-crate changes, run:

```bash
VALIDATION_LOG=$(mktemp /tmp/rho-validation-full.XXXXXX.log)
python3 scripts/validate.py full >"$VALIDATION_LOG" 2>&1
```

Full mode runs policy and script checks, Clippy for all workspace targets and features, normal workspace tests, documentation tests, SDK feature and downstream checks, and the docs TUI proof-plate PTY check. Do not precede it with a separate workspace `cargo check`; Clippy and tests already provide that compile coverage. Do not add `--all-targets` to the normal workspace test command; Clippy, platform CI, and the dedicated benchmark job cover examples and benchmarks.

Do not raise architecture line budgets merely to pass. Extract cohesive modules instead. Full mode includes the architecture self-test, so run that self-test separately only when changing the checker and not running full mode.

For interactive TUI behavior, load and follow `rho-tui-pty-testing` first. Run the PTY smoke suite or a named scenario when the change touches interactive flows. Fall back to `rho-tui-herdr-testing` only for exploratory validation or when a PTY scenario cannot cover the behavior yet. Record the path used and its result here.

When the change affects Interactive TUI layout, chrome, tool cards, statusline, version display, the docs proof-plate fixture, or `rho-pty-demo`, also run:

```bash
bash scripts/check_docs_ui_demo.sh --check
# on drift:
bash scripts/check_docs_ui_demo.sh --write
```

Commit both `docs/assets/rho-ui-demo.svg` and `docs/public/assets/rho-ui-demo.svg` after `--write`. CI job `docs TUI proof plate` enforces the check on every PR.

## 4. Handle failures

Inspect focused excerpts, for example:

```bash
tail -n 80 "$VALIDATION_LOG"
rg -n "error|failed|failure|panicked|warning" "$VALIDATION_LOG" | tail -n 80
```

Classify failures as caused by the change, an adjacent issue to fix, unrelated pre-existing work, or environmental. Fix obvious adjacent issues when safe. Do not weaken tests, increase budgets, add broad allows, or silently skip checks to obtain a pass.

## 5. Review the final state

```bash
git status --short
git diff --check
git diff --stat
git diff
git diff --cached --check
```

Verify only intended files changed, tests cover behavior rather than trivia, APIs and state transitions are intentional, important user-visible changes are documented, and generated files such as `CHANGELOG.md` were not manually edited.

If committing, use the repository's Conventional Commit format. Keep the description imperative and lowercase, with no final period. Mark breaking changes with `!` and a `BREAKING CHANGE:` footer.

## 6. Report

Report:

- behavior and code reviewed
- exact fast or full workflow, package, test target, lint, and smoke-test commands run
- pass, failure, or blocked status for each
- fixes made during validation
- checks not run and why
- useful temporary log paths

Do not imply unrun checks passed or dump full logs.
