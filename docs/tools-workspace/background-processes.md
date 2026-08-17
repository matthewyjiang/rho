# Background processes

Parent: [Tools and workspace](/tools-workspace).

The `process` tool runs a shell command in the background inside the current
Rho instance. Start it, poll retained output with a cursor, and stop it when
done. Use this for long-running servers, watchers, and other work that should
outlive a single foreground `bash` or `powershell` call.

Rho owns these processes only while that instance is alive. Shutdown cleans
them up. Records do not survive a restart. The interactive TUI shows live
`starting` and `running` jobs in the activity rail. That host view is not a
`process` tool `list` action.

```mermaid
flowchart TD
    start["start: command"] --> id[process_id]
    id --> poll["poll: cursor + optional wait"]
    poll --> out[Retained stdout and stderr]
    out --> more{Need more output?}
    more -->|yes| poll
    more -->|stop or done| stop["stop: process_id"]
    stop --> end[Process tree terminated]
    id --> stop
```

## Actions

| Action | Required | Optional | Result |
| --- | --- | --- | --- |
| `start` | `command` | `timeout_seconds` (≥ 1) | Compact snapshot with `process_id` and early output |
| `poll` | `process_id` | `cursor`, `wait_seconds` (0–30) | Compact snapshot of retained output from the cursor |
| `stop` | `process_id` | | Stop request for that managed process tree |

### `start`

Launches the command through the platform shell with:

- working directory set to the workspace root
- stdin closed
- stdout and stderr captured on pipes
- the same user permissions as Rho

Returns a compact text snapshot that includes `process_id`. Optional
`timeout_seconds` bounds how long the process may run before Rho marks it timed
out and stops the tree.

### `poll`

Reads retained stdout and stderr. Pass the previous `next_cursor` as `cursor` so
you do not re-read the same chunks.

- `wait_seconds` may block briefly (0–30, default 0) for new output.
- Retention is bounded. If the requested cursor is older than the retained
  range, the snapshot reports that and advances from what is still held.
- `output_pending` is true when more retained output exists past the returned
  window.
- `truncated` is true when output was dropped under the byte or chunk caps.

### `stop`

Requests termination of the managed process tree (process group on Unix, job
object on Windows), not only the direct child. Rho waits a short grace period,
then force-kills if needed.

## Snapshot fields

`start` and `poll` return compact text, not JSON. Typical lines:

| Line | Meaning |
| --- | --- |
| `process_id: …` | Handle for later `poll` / `stop` |
| `state: …` | `starting`, `running`, `exited`, `terminated`, `timed_out`, or `failed_to_start` |
| `next: …` | Pass this as `cursor` on the next `poll` |
| `truncated: first=…` | Requested cursor is older than the retained range |
| `pending` | More retained output exists past this window |
| `exit: …` | Non-zero or failed exit code |
| `detail: …` | Extra detail for failed or forced ends |
| `stdout:` / `stderr:` | Coalesced stream text; omitted when empty |

## Limits

Default manager limits (per Rho instance):

| Limit | Default |
| --- | --- |
| Live processes | 16 |
| Retained process records | 64 |
| Retained output per process | 1 MiB |
| Retained chunks per process | 8,192 |
| Completed-record retention | 30 minutes |
| `poll` wait | 0–30 seconds |
| `stop` grace | 2 seconds |

Rho prunes completed records past retention and drops the oldest completed
records when the record cap is hit. Live processes count against `max_live`.

## What this tool is not

The `process` tool does not provide:

- stdin writes after start
- process listing as its own action
- a pseudo-terminal or interactive TUI inside the child
- persistent sessions across Rho restarts
- pane or session orchestration

Commands that need a real terminal, interactive prompts, or durable attachable
sessions belong in a multiplexer such as tmux or Herdr, or in a foreground
`bash` / `powershell` call when the work fits one turn.

## Permissions and safety

`process` requests the Process capability. [Permission modes](/configuration#permission-modes)
can deny it (`plan`), classify it (`auto`), or ask first (`allow_edits` and
`supervised`). The `allow_edits` free-write skip covers workspace file writes
only; process execution always needs approval. They do not add an
operating-system sandbox. The child still runs with the current user's rights
and can affect files inside or outside the workspace the same way a shell in
that account could.

Foreground `bash` and `powershell` are separate tools for work that should
finish inside one tool call. Prefer `process` when you need to keep a command
running across turns and read its output later.

## Related

- [Tools and workspace](/tools-workspace) - workspace root and capability model
- [Permission modes](/configuration#permission-modes) - plan, auto, allow_edits, and supervised
  process policy
- [Workflow runtime](/workflows/runtime) - host-only `workflow_command` for
  frozen workflow steps (not the agent `process` tool)
