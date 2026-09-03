# Cursor stream-json fixtures

NDJSON captures of `cursor-agent -p --output-format stream-json
--stream-partial-output` for deterministic mapper tests. All four are live
captures from `cursor-agent 2026.08.25-3e8eec8` with model `composer-2.5`
on 2026-09-03, sanitized (not byte-for-byte): session ids replaced with
stable fixture UUIDs, `request_id` / `model_call_id` / `call_id` replaced with
`req-NNNN` / `tool_NNNN`, `timestamp_ms` pinned, repo and home paths rewritten
to `/tmp/rho-cursor-fixture-*`, `conversationId` / `requestId` /
`hookAdditionalContexts` / `contentBlobId` dropped or pinned. Frame order,
delta boundaries, snapshot frames, tool args, and tool results are unchanged.

| File | What it exercises |
| --- | --- |
| `live_text_thinking.ndjson` | Prompt on stdin, `--single-turn`. 12 `thinking/delta` + `completed`, 218 `assistant` deltas, then one cumulative snapshot frame (no `timestamp_ms`) equal to the concatenated deltas, then `result/success` with usage. |
| `live_edit.ndjson` | Default `-p` (no `--force`, no allow list): `editToolCall` started/completed with `result.success.diffString`, `linesAdded`, `linesRemoved`. Proves `-p` writes without approval. |
| `live_shell_mid_snapshot.ndjson` | Captured with `--force --exclude-tools shell_tool_call`; shell ran anyway. Text segment → mid-turn snapshot carrying `model_call_id` → `shellToolCall` with `exitCode`, `stdout`, `executionTime` → second text segment → final snapshot. The mapper's segment-reset rule is tested here. |
| `live_readonly_search.ndjson` | `--allowed-tools read_tool_call,grep_tool_call,glob_tool_call,ls_tool_call`: `globToolCall` (`files`, `totalFiles`), `grepToolCall` (`workspaceResults` tree), `readToolCall` (`totalLines`). Model reports it cannot edit. |

Keep fixtures deterministic. Do not commit credentials or private content.
Automated tests only read fixtures; they never execute `cursor-agent`.
