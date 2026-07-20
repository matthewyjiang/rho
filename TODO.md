# Architecture refactor

## Implementation

- [x] Parse built-in agent tool declarations once into typed capabilities.
- [x] Move legacy SDK tool adapters from the application registry into `rho-tools`.
- [x] Split application tool construction into feature-owned bundles with explicit lifecycle ownership.
- [x] Extract session persistence and canonical session ID resolution behind `SessionStore`.
- [x] Split interactive run, session, and provider concerns into focused controllers/state modules.
- [x] Expose provider-owned authentication dispatch so the TUI does not import concrete login functions.
- [x] Reduce the TUI root by extracting narrow action services and bootstrap/view data where practical.

## Validation

- [x] Add or update focused tests for each changed boundary.
- [x] Run `cargo fmt`.
- [x] Run `python3 scripts/check_architecture.py`.
- [x] Run focused crate tests.
- [x] Run TUI PTY smoke tests.
- [x] Run workspace Clippy/tests as warranted by the final diff.
- [x] Resolve all blocking reviewer-agent findings.
