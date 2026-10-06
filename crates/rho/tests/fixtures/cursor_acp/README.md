# Cursor ACP E2E fixtures

Inputs for the offline `fake_cursor_acp_runtime_end_to_end` PTY scenario in
`tests/tui_pty.rs`. A `cursor-agent` shim on PATH records argv, cwd, and
`CURSOR_CONFIG_DIR`, then execs the debug-only `rho __acp-fixture-agent`, which
plays `success.json` over ACP stdio.

- `cursor-worker.md`: agent declaring read, grep, and shell. The fence denies
  writes, and web search must be rejected even though grep (same ACP kind
  `search`) is declared.
- `success.json`: one turn. A shell tool call, a shell permission request
  (expect allow once), a Cursor-shaped web-search permission request (kind
  `search`, `web_search_` id; expect reject), the shell completion, final text
  `rho-cursor-acp-e2e-ok`, then `end_turn`. Shapes follow the live recordings
  in `crates/rho/src/acp_runtime/fixtures/`.
