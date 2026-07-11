---
name: rho-rust-change-validation
description: Validate and finalize Rust changes in Rho. Use after editing Rust, before committing or opening a PR, when fixing test or lint failures, or when reviewing compliance with Rho's architecture and test conventions. Review the diff, run focused checks, and report exactly what was validated.
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

- Prefer behavior or integration tests for user-visible behavior and unit tests for focused pure logic.
- Put new test modules in sibling `*_tests.rs` files with an explicit `#[path = "..."] mod tests;` when practical.
- Prefer `pretty_assertions::assert_eq` and whole-object comparisons when available.
- Do not test static constants or removed behavior.
- Avoid mutating process environment; inject environment-derived values or dependencies instead.

For bug fixes, reproduce the issue through the closest practical user path before finalizing when feasible.

## 3. Run validation

Capture verbose output in temporary logs and inspect only relevant excerpts.

### Required after Rust changes

```bash
cargo fmt

ARCH_LOG=$(mktemp /tmp/rho-architecture.XXXXXX.log)
python3 scripts/check_architecture.py >"$ARCH_LOG" 2>&1

TEST_LOG=$(mktemp /tmp/rho-tests.XXXXXX.log)
cargo test <focused-filter-or-target> >"$TEST_LOG" 2>&1
```

Choose the test target from the changed modules rather than copying the placeholder. Run broader `cargo test` only when changes cross boundaries or focused coverage is insufficient.

Do not raise architecture line budgets merely to pass. Extract cohesive modules instead. Run `scripts/check_architecture.py --self-test` only when changing the checker, its policy, related documentation, or fixtures.

Run `cargo check` or `cargo clippy --all-targets --all-features` when requested or when they add meaningful coverage. Never claim a check passed unless it was run.

For interactive TUI behavior, load and follow `rho-tui-herdr-testing`. That skill owns the Herdr workflow, interaction guidance, model-cost policy, assertions, and cleanup. Record its flow and result here.

## 4. Handle failures

Inspect focused excerpts, for example:

```bash
tail -n 80 "$TEST_LOG"
rg -n "error|failed|failure|panicked|warning" "$TEST_LOG" | tail -n 80
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
- exact formatting, architecture, test, lint, and smoke-test commands run
- pass, failure, or blocked status for each
- fixes made during validation
- checks not run and why
- useful temporary log paths

Do not imply unrun checks passed or dump full logs.
