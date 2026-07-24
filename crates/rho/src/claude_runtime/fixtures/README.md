# Claude stream-json fixtures

NDJSON shapes for deterministic unit tests. Most files are hand-authored from
the terminal `result` schema in `PLAN.md` and the partial-message / tool
envelopes the adapter must accept. `live_success.ndjson` is a sanitized live
capture from Claude Code.

| File | Provenance |
| --- | --- |
| `success.ndjson` | Hand-authored from the PLAN.md high-cache terminal example (`input_tokens=2`, `cache_read=7294`, `cache_creation=6095`, `contextWindow=200000`). Not a live capture. |
| `live_success.ndjson` | Live capture, 2026-07-24, Claude Code `2.1.219` (Pro / claude.ai). Prompt: `reply exactly \`rho-claude-e2e-ok\``. Capture args matched production spawn: `claude -p --output-format stream-json --verbose --include-partial-messages --permission-mode dontAsk --tools "" --disallowedTools Task --setting-sources project --strict-mcp-config --max-turns 1`, prompt on stdin, isolated empty cwd (no project CLAUDE.md). Sanitized (not byte-for-byte): real `session_id` / message id / request id / envelope uuids replaced with stable fixture UUIDs of the same shape; cwd and `memory_paths` rewritten to `/tmp/rho-claude-fixture-*`; init `slash_commands` / `agents` / `skills` emptied (machine-specific capability/config lists); rate-limit `resetsAt` / `overageResetsAt` and assistant `timestamp` normalized to stable placeholders; no email, org id, or home-directory path retained. Message structure, partial deltas, rate-limit event shape, usage, and `modelUsage` kept; response text remains `rho-claude-e2e-ok`. |
| `tool_call.ndjson` | Hand-authored tool_use / tool_result envelopes. Not a live capture. |
| `error_result.ndjson` | Hand-authored `error_max_turns` + `permission_denials`. Not a live capture. |
| `unknown_and_partial.ndjson` | Hand-authored unknown type + text-only partial deltas. Not a live capture. |
| `mixed_partial_complete.ndjson` | Hand-authored mixed stream: `--include-partial-messages` deltas, complete assistant envelope, tool use/result, and terminal result. Not a live capture. |
| `partial_tool_complete_text.ndjson` | Hand-authored: tool streamed via partials; thinking/text only on complete envelope. Not a live capture. |
| `message_start_no_deltas.ndjson` | Hand-authored: `message_start`/`message_stop` with no content deltas; complete envelope supplies all blocks. Not a live capture. |
| `missing_message_id.ndjson` | Hand-authored: two complete assistants without message ids must not share a fallback key. Not a live capture. |

Do not invent "synthetic capture" labels. Keep fixtures deterministic. Do not
commit credentials or private content. Live multi-turn / tool streams were not
recaptured here (quota: at most two live requests; second request reserved for
adapter E2E). Mixed/tool behavior stays covered by the hand-authored fixtures
above.
