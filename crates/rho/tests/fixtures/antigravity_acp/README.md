# Antigravity ACP E2E fixtures

Inputs for the offline `fake_antigravity_acp_runtime_end_to_end` PTY scenario
in `tests/tui_pty.rs`. An `agy` shim on PATH records argv and cwd, then execs
the debug-only `rho __acp-fixture-agent`, which plays `success.json` over ACP
stdio.

- `antigravity-worker.md`: agent pinned to `gemini-3.8-flash` with tools
  `read_file` and `run_command`.
- `success.json`: advertises agy 1.5.0's modes (`default | accept-edits |
  plan`) and its `model` and `thought_level` select options (current model
  `gemini-4-argon`). One turn: a `run_command` call and its permission request
  (kind `execute`; expect `allow` in Bypass), the command result, final text
  `rho-antigravity-acp-e2e-ok`, then `end_turn`. Request shapes and option ids
  follow live `agy acp` 1.5.0 recordings (2026-11-04).
