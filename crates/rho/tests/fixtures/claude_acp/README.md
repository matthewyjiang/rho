# Claude Code ACP E2E fixtures

Inputs for the offline Claude runtime PTY scenarios in `tests/tui_pty.rs`. A
`claude-agent-acp` shim on PATH records argv and cwd, then execs the
debug-only `rho __acp-fixture-agent`, which plays a script over ACP stdio. The
shim answers `--cli auth status` and `--cli --version` itself (see
`tests/support/claude_e2e.rs`).

- `claude-planner.md`: agent pinned to `claude-opus-demo` with tools `Read`,
  `Edit`, and `Bash(git *)`, replacing the system prompt.
- `success.json`: advertises claude-agent-acp 0.100.0's modes and steering.
  One turn: a `Read` call and its result, a `usage_update` with the resolved
  model (`claude-sonnet-5`) and a rate-limit event under `_meta`, the final
  text split as `r` + `ho-claude-e2e-ok`, a `usage_update` with the session
  cost (`0.034271` USD), then `end_turn`. Shapes follow live claude-agent-acp
  0.100.0 recordings (2026-11-04). The delegated runs and the advisor both
  play it.
- `error.json`: partial text, then `max_turn_requests`.
