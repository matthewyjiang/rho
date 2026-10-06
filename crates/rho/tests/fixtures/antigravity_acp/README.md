# Antigravity ACP E2E fixtures

Inputs for the offline `fake_antigravity_acp_runtime_end_to_end` PTY scenario
in `tests/tui_pty.rs`. An `agy_acp_server.par` shim on PATH (with a sibling
`localharness_external`) records argv, cwd, and `ANTIGRAVITY_HARNESS_PATH`,
then execs the debug-only `rho __acp-fixture-agent`, which plays
`success.json` over ACP stdio. The isolated home holds a signed-in
`~/.gemini/antigravity-acp/` so the sign-in preflight passes.

- `antigravity-worker.md`: agent declaring `view_file` and `run_command`,
  pinned to `gemini-3.8-flash-low`.
- `success.json`: advertises agy_acp_server 1.3.0's modes
  (`default | auto_edit | yolo`) and `model` select option (current
  `gemini-3.8-flash-high`). One turn: a `run_command` call and its permission
  request (kind `execute`; expect `allow`), a `read_url_content` permission
  request (kind `fetch`, undeclared; expect `deny`), the command result, final
  text `rho-antigravity-acp-e2e-ok`, then `end_turn`. Request shapes and option
  ids follow live agy_acp_server 1.3.0 recordings (2026-10-06).
