# Activity rail

Parent: [Interactive TUI](/interactive-tui).

While a model turn, background `agent` run, or `process` job is live, Rho keeps a spinner at the bottom of the transcript and hangs rail rows off it as one tree. The rail stays visible in zen mode.

The spinner aggregates parent work and background counts, for example `running tool · 2 agents · 1 job · 1m 12s`. When the parent turn is idle but background work remains, it stays up as `1 job running`, `2 agents working`, or `2 agents · 1 job`. On narrow widths the label drops elapsed time first, then compresses.

The rail shows at most two subagent rows and two process rows.

| Row | Shows | Action |
| --- | --- | --- |
| Subagent | Role, generated title, current tool or action, elapsed | Click to attach. Hover shows `⏎ attach` and keeps the timer |
| Process | Command, freshness, and elapsed. No process id | Click to peek captured output. Read-only, no stop. Hover shows `⏎ peek` |
| Overflow | `2 more agents · /attach` or `1 more job` | Replaces the last row when more runs are live than fit |

A process peek replaces the session with that job's captured stdout and stderr. The parent session keeps running. Up, Down, Page Up, Page Down, Home, and End scroll. `q` or Escape returns. There is no stop or kill from this view.

Process freshness is `running` while output is recent, then `quiet 4m 12s` after 60 seconds of silence. Past five minutes of silence the elapsed column tints as a warning.

Finished rows linger with a verdict, then the rail shrinks in one repaint. Success holds a few seconds. Failures hold longer so a failing background process does not vanish.

| Kind | Verdicts |
| --- | --- |
| Agent | `✓ done`, `✗ error`, `✗ stopped` |
| Process | `✓ exit 0`, `✗ exit 101`, `✗ timed out`, `✗ terminated`, `✗ failed to start` |

## Watch a subagent

`/attach`, a rail click, or `rho attach` opens a read-only view of a delegated run. The parent session keeps running. The view shows the delegated prompt, reasoning, assistant output, tool activity, usage, and final state. It has no message box and cannot submit prompts or change the subagent environment.

```bash
rho attach
rho attach abc123
```

`rho attach` with no id opens a picker of subagents from the current directory. It starts on running runs. Ctrl-R includes finished transcripts. `/attach` does the same inside the TUI.

In the in-place view, Up, Down, Page Up, Page Down, Home, and End scroll. Tab, Shift-Tab, Left, and Right cycle other running subagents. Click a truncated tool card, or press Ctrl+O, to expand or collapse it. `q` or Escape returns to the composer. Ctrl-C quits Rho. If the parent hits an approval, questionnaire, or turn completion while you are attached, the footer notes it. The view does not yank you back.

`rho attach` and `rho attach <id>` open that same view in a separate process, for another terminal. For Claude-cli runs, attach also shows `claude_session_id` when present so you can open the Claude transcript with `claude --resume <session-id>`. Storage and lifecycle: [Attachment and artifacts](/subagents/attachment-and-artifacts).
