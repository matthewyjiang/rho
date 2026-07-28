---
name: rho-test-selection
description: Choose, write, review, and delete Rho tests under the suite quality gate. Use when adding tests, expanding a suite, reviewing test-only PRs, deleting low-value or flaky tests, deciding unit vs PTY vs SDK coverage, or enforcing failure-mode and owner-layer rules.
compatibility: Applies to the Rho monorepo test layout and PR template.
---

# Rho test selection

Every test must earn its place. Prefer deleting or merging weak tests over growing the suite.

Load this skill before adding `#[test]` / `#[tokio::test]`, before large test edits, and when reviewing test growth. For interactive TUI scenario mechanics, also load `rho-tui-pty-testing`. For Herdr exploratory checks, load `rho-tui-herdr-testing`.

## Gate before you add a test

Answer all three:

1. **Failure mode** - What user-visible or contract bug does this catch if it fails?
2. **Owner layer** - Which single layer owns that failure mode (see below)?
3. **Gap** - What existing test does *not* already cover it?

If you cannot name a distinct failure mode and one owner layer, do not add the test.

Put a short header on non-obvious new tests:

```rust
// Covers: cancel mid-tool must not commit partial results
// Owner: sdk orchestration
#[tokio::test]
async fn cancel_mid_tool_does_not_commit_partial_results() { ... }
```

Pull requests that add or materially expand tests must fill the **Test gate** section in `.github/pull_request_template.md`.

## One failure mode, one owner layer

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

## Keep vs reject

### Prefer (Tier A)

- Invariants that can regress silently: cancel, retry, resume, migration, replay, tool pairing
- Security and privacy: permissions, SSRF, credential storage, redaction
- Public SDK or provider wire contracts that break real backends
- PTY coverage for startup, submit, interrupt, pickers, resize, paste, shutdown
- Pure parsers and policy as **table-driven** unit tests

### Keep thin (Tier B)

- Config migrations and invalid-config errors
- One structured test per rule, with cases in a table
- OS-specific path or shell edges

### Default reject (Tier C)

- Happy-path tests whose only failure mode is "someone deleted the feature"
- Another case of the same branch with different literals as a new function
- Rendered chrome, labels, badges, help text, statusline copy, or theme strings
- `Default` / serde default checks that only restate derives
- Field-by-field load/save tests for each config key when one table or migration test would do
- Negative tests whose only purpose is to document removed behavior
- Static constants
- Private helper wiring already covered by a higher-layer test
- Duplicate coverage of the same branch at unit + integration + PTY
- Wall-clock sleeps used to synchronize tasks, known flakes, and timing races (see Determinism)

## Determinism

Tests must be deterministic under parallel `cargo test` and slow CI.

### Reject

- `std::thread::sleep` / `tokio::time::sleep` used so "the other task can finish"
- Polling loops with fixed delays instead of an explicit condition
- Tests that fail under load or need retries / longer sleeps to pass
- Parking known flakes with `#[ignore]` without a rewrite or delete plan

### Allow

- Wait on an explicit signal: channel, `Notify`, `Barrier`, `Atomic*`, or a condition the code under test owns
- `timeout(...)` as a **failure bound** around that signal, not as the sync mechanism
- PTY/harness waits on observable output (`wait_for_text`, exit) with a named timeout
- Fake or manual clocks when the product logic is time-based

### If a test is flaky

1. Rewrite to event-driven or pure inputs when the failure mode still matters.
2. Delete when a deterministic test already covers it, or rewrite cost exceeds value.
3. Do not keep it by lengthening sleeps or ignoring it indefinitely.

## How to write tests you keep

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

## Interactive TUI

PTY is the product gate for interactive behavior. Unit tests under `crates/rho/src/tui` stay limited to pure logic.

- harness crate: `crates/rho-tui-pty`
- smoke tests: `cargo test -p rho-coding-agent --test tui_pty`
- single scenario: `cargo run -p rho-tui-pty --bin rho-pty-scenario -- --bin target/debug/rho <scenario>`
- skill: `rho-tui-pty-testing`

For interactive bugfixes, add or extend a PTY scenario by default. Do not add a large buffer-string unit test for the same bug unless the logic is pure and better expressed as a table.

Use Herdr sibling-pane smoke tests only for exploratory validation, novel bugs not yet covered by a scenario, or real-terminal parity checks (`rho-tui-herdr-testing`).

## Deleting and reviewing tests

When shrinking the suite or reviewing test-only growth, delete or demand changes for:

- Tier C tests without a distinct failure mode
- TUI unit tests for interactive behavior without a PTY scenario or a clear reason PTY is wrong
- Many near-duplicate functions instead of a table
- Copy or chrome string locks
- Sleep-synced or known-flaky tests
- Material test LOC growth without deleting or tightening anything nearby

Prefer one strong owner-layer test over the same branch covered three ways.

## Related skills

- `rho-tui-pty-testing` - write and run PTY scenarios
- `rho-tui-herdr-testing` - exploratory real-pane checks
- `rho-rust-change-validation` - format, architecture, and test execution gates after Rust edits
