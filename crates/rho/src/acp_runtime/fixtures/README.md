# Cursor ACP spike recordings

Recorded 2026-10-05 against `cursor-agent 2026.10.01-e373342` (`cursor-agent --trust
--model composer-2.5 acp`), API-key auth (`CURSOR_API_KEY`), Linux x86_64. A Python
ACP client logged every wire message as `{"dir", "ms", "msg"}` rows: `c2a` is
client→agent, `a2c` is agent→client, `ms` is monotonic milliseconds since spawn,
and the last row is a timing summary. To keep the files small, model lists and
`available_commands_update` payloads are trimmed, workspace paths are rewritten
to `/workspace`, and the cancel recording keeps only the first 12 chunks.

| File | Scenario |
|---|---|
| `cursor_agent_tools.jsonl` | Agent mode, user `approvalMode: unrestricted`: read, grep, shell, two edits. No permission requests |
| `cursor_permission_reject.jsonl` | `approvalMode: allowlist`, empty allow list, client answers `reject-once`: shell asks, read and edit do not |
| `cursor_plan_mode.jsonl` | `session/set_mode plan` + allowlist + reject: edit blocked by mode, shell asks then blocked |
| `cursor_cancel.jsonl` | `session/cancel` mid-turn |
| `cursor_deny_read_fetch.jsonl` | `allowlist` + deny `Read(**)`, `WebFetch(*)`: read is denied ("Permission denied", reported `completed`); grep and glob still run; fetch **ignores** the deny and asks via `session/request_permission` (`kind: fetch`, `toolCallId: web_fetch_0`) |
| `cursor_ext_update_todos.jsonl` | `cursor/update_todos` arrives as a JSON-RPC request with no `sessionId` |

## Checklist answers

1. **Flags before `acp`.** `--model` is honored (`session/new` reports the
   `currentModelId`; `composer-2.5` resolved to `composer-2.5[fast=true]`).
   `--mode plan` is ignored: `currentModeId` stayed `agent` and every tool ran.
   `--allowed-tools read_tool_call` is ignored: shell and edit both ran. Plan
   mode has to go through `session/set_mode` (answers `{}` and emits
   `current_mode_update`). Cursor also implements `session/set_config_option`
   and `unstable_setSessionModel`.
2. **Permission requests depend on the user's Cursor config, not on ACP.** With
   the default `approvalMode: unrestricted` (Run Everything), nothing asks.
   With `allowlist`, only shell asks (`kind: execute`); reads and file edits
   never ask, even with an empty allow list. Plan mode (via `set_mode`) blocks
   edits but **not** shell: under `unrestricted`, plan mode ran
   `echo > shell.txt`. Cursor's `permissions.deny` (`Write(**)`, `Shell(*)`)
   does fence (`Read(**)` too); `WebFetch(*)` does **not**, but under
   `allowlist` fetch asks permission, so rejection fences it. Grep and glob are
   not covered by `Read(**)`. Denied and rejected tools then report `status: completed` with no
   output, not `failed`. `allow-always` persists into Cursor's allowlist
   (source), so Rho must never pick it. The config dir is
   `$CURSOR_CONFIG_DIR`, else `$XDG_CONFIG_HOME/cursor`, else `~/.cursor`, and
   it holds `cli-config.json`.
   - Tool shapes: `tool_call` carries `kind` (`read`/`search`/`execute`/`edit`/`other`),
     `title`, `rawInput` (`{path}`, `{command}`, `{pattern,path}`), `locations`.
     Edit and search calls start with an empty `rawInput`, which a
     `tool_call_update` fills in. Results: read → `rawOutput.content`; shell →
     `rawOutput.{exitCode,stdout,stderr}`; grep → `rawOutput.{totalMatches,truncated}`;
     edit → `content: [{type: diff, path, oldText, newText}]`. A new-file diff
     uses `oldText: "-- /dev/null"` and prefixes `newText` with a `++ b/<path>`
     header line. Todos show up as `kind: other`, `rawInput._toolName: updateTodos`.
3. **Auth.** `authenticate(cursor_login)` errors immediately (`-32602`, "Failed
   to open browser for login", plus a login URL) when only an API key is
   present. `session/new` works without `authenticate`, and when
   unauthenticated it returns an `authRequired` error rather than hanging
   (source). So Rho should not call `authenticate`. `cursor-agent status
   --format json` reports `unauthenticated` even when `CURSOR_API_KEY` works.
4. **Session ids.** `cursor-agent --resume <acp sessionId> -p …` did not see
   the ACP conversation. The ACP `sessionId` is not a `--resume` chat id.
5. **Extensions.** `cursor/update_todos`, `cursor/task` and `cursor/generate_image`
   are sent as JSON-RPC **requests** (with an `id`, no `sessionId`), and the
   agent ignores errors on them (source: `sendNonBlockingExtensionNotification`
   → `extMethod(...).catch`). `cursor/ask_question` and `cursor/create_plan` are
   blocking `extMethod` requests. In the source, `ask_question` can also come in
   as `session/request_permission` with one `allow_once` option per answer and
   a `__ask_question_skip__` `reject_once` option. The live run never triggered
   `ask_question`. Undocumented extras in the source: `cursor/canvas`,
   `cursor/list_available_models` (client→agent), and non-standard
   `subagent_spawned` / `subagent_state_update` session updates, sent only when
   the client advertises a subagents capability.
6. **Cancel and exit.** `session/cancel` produced `stopReason: cancelled` 4 ms
   later. After a prompt turn, the process **never** exited on stdin EOF within
   10 s (10/10 runs). Idle sessions exited in about 564 ms (3/4). Rho must
   kill the child after the turn.
7. **Usage.** No `usage_update` in any recording (`sessionUpdate` kinds seen:
   `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`,
   `available_commands_update`, `session_info_update`, `current_mode_update`).
   The source never emits `usage_update`. Token and cost totals regress
   compared with `-p`.
8. **Handshake latency** (n=14): `initialize` 420–503 ms; `session/new`
   2411–3381 ms. A 10 s per-step bound is about 3x the worst observed.
