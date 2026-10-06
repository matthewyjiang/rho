# Cursor fixtures

`live_models.txt` is a live capture of `cursor-agent models` from
`cursor-agent 2026.09.02` (plain text, no `--format`). 221 lines, 217 models.
The list is per-account. Tests parse it; they never execute `cursor-agent`.

ACP wire recordings for the delegated-run path live in
`crates/rho/src/acp_runtime/fixtures/`.

Keep fixtures deterministic. Do not commit credentials or private content.
