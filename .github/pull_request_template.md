## Summary

<!-- What changed and why. -->

## Validation

<!-- Commands run and results. Prefer the narrowest relevant tests, then broader gates when the change warrants them. -->

```text
```

## Test gate

<!-- Required when this PR adds or materially expands tests. Delete this section only if the diff adds no tests. Full rules: rho-test-selection skill. -->

- [ ] Followed `rho-test-selection` (failure mode, owner layer, gap).
- [ ] Each new test names a distinct **failure mode** (user-visible or contract bug).
- [ ] Each new test has one **owner layer** (pure unit / SDK contract / PTY / OS).
- [ ] No existing test already covers that failure mode at a better layer.
- [ ] Interactive TUI behavior uses a **PTY scenario** by default; new `crates/rho/src/tui` unit tests are pure logic or justified below.
- [ ] Cases share one test function per rule (tables), not twin functions per literal.
- [ ] Asserts use structured values; string `.contains` only for redaction, wire format, or security escaping.
- [ ] No locks on help text, statusline chrome, labels, or other copy.
- [ ] No wall-clock sleep used for synchronization; no known-flaky timing races.
- [ ] Nearby weaker or duplicate tests were removed or merged when practical.

### New tests

| Failure mode | Owner layer | Why existing coverage is not enough |
| --- | --- | --- |
| | | |

### PTY exception (if any)

<!-- If you added interactive TUI unit tests instead of or beside PTY, say why PTY is the wrong layer. -->

```text
N/A
```

## Breaking changes

<!-- Delete if none. Otherwise describe the break and migration. -->
