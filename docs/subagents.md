# Subagents

Rho can delegate work to subagents: separate `rho run` processes spawned by
the model through the `agent` tool. Inside [herdr](https://herdr.dev), each
subagent runs in its own pane so you can watch and scroll it live; outside
herdr it runs headless with output teed to a log file. Either way, results
flow back to the parent through a structured result file — the parent never
reads pane or log output, which keeps delegation cheap in tokens.

## Presets

A preset defines a reusable subagent configuration: the model, reasoning
level, allowed tools, and extra system-prompt instructions. Presets are
markdown files with YAML frontmatter:

```markdown
---
description: Reviews diffs for correctness bugs
model: gpt-5.5
reasoning: high
tools: [read_file, list_dir, bash]
on_exit: close-on-success
---
You are a code review subagent. Never modify files.
Report findings as file:line references with a one-line explanation.
```

The file name (without `.md`) is the preset name. Presets are discovered
from, in order of precedence:

1. `~/.rho/agents/*.md`
2. `~/.agents/agents/*.md`
3. `.agents/agents/*.md` in the project (nearest project wins)

Two presets ship built in — `explorer` (a fast read-only scout) and `worker`
(full tool set) — and a user file with the same name overrides them.

### Frontmatter fields

| Field | Required | Meaning |
| --- | --- | --- |
| `description` | yes | Shown to the model when choosing a preset |
| `model` | no | Model override; inherits the parent config when unset |
| `provider` | no | Provider override |
| `reasoning` | no | `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` |
| `tools` | no | Allowed tool names; unset grants the full tool set |
| `on_exit` | no | Herdr pane behavior: `keep` (default), `close`, `close-on-success` |

The markdown body is appended to the subagent's system prompt.

The `tools` list is the subagent's permission boundary: tools not listed are
never registered, so the subagent cannot call them.

## The agent and agents tools

The model spawns subagents with the `agent` tool:

- **Blocking** (default): the tool call resolves when the subagent finishes,
  returning its final answer, turn count, and token usage.
- **Background** (`background: true`): the call returns immediately with a
  short id. When the subagent finishes, the parent is notified at its next
  turn boundary — an idle interactive session is woken with the result.

The `agents` tool manages running subagents:

- `list` — all subagents spawned this session
- `status` — state, elapsed time, turns, token usage, and last activity for
  one subagent
- `stop` — graceful stop (the subagent writes a partial result), escalating
  to a kill after five seconds

Pass `--no-subagents` to hide both tools. Subagents themselves always run
with `--no-subagents`, so they cannot spawn further subagents.

## Where things live

Each spawn gets a directory under `~/.rho/subagents/<id>/` containing
`result.json` (the live status/result contract), `cancel.requested` when a stop
has been requested, and, for headless spawns, `log.txt`. The cancellation marker
is cross-platform; after requesting it, the parent allows five seconds for a
partial result before force-killing the process. Inside herdr the subagent's
output is visible in its pane instead.

## Running a subagent by hand

The same machinery is available directly from the CLI:

```bash
rho run --preset explorer --output-file /tmp/result.json "where is auth handled?"
```

With `--output-file`, progress streams to stdout and the JSON file is updated
during the run (state, pid, turns, token usage, last activity) and finalized
on exit with `state` of `ok`, `error`, or `stopped` plus the final `result`
text.
