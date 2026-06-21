# Interactive TUI plan

Rho's primary interface is an interactive Ratatui application. The command-line interface remains for non-interactive automation only.

## Product direction

- `rho` opens an inline terminal UI when stdin and stdout are terminals.
- `rho run ...` executes one prompt and exits for scripts, hooks, aliases, and CI jobs.
- The CLI does not provide a REPL. There is one interactive surface: the TUI.

## Current TUI layout

- Finalized conversation output is inserted into normal terminal scrollback.
- There is no bordered conversation pane and no internal conversation scroll state.
- The active assistant response and reasoning deltas render inline above the composer while a turn is running.
- The composer is a wrapping prompt between simple horizontal divider lines, with no section labels.

## Current keybindings

- `enter`: send the current prompt.
- `shift-enter`: insert a newline when the terminal reports it distinctly.
- `alt-enter`: insert a newline fallback.
- paste: insert pasted text, including newlines, without submitting. Rapid paste-like key streams are also grouped so pasted `enter` events become newlines instead of submissions.
- arrow keys: move within the prompt.
- `alt-left` / `alt-right`: move by word.
- `alt-backspace`: delete the previous word.
- `home` / `end`: jump to the start or end of the prompt.
- `esc`: interrupt a streaming response without quitting the app.
- mouse wheel: scroll terminal history.
- `ctrl-r`: reset conversation history.
- `ctrl-c`: clear the input line, press twice to quit.

## Automation mode

Use `rho run` for non-interactive calls:

```bash
rho run "summarize this repository"
printf 'summarize this repository' | rho run --stdin
rho run "review this diff" --stdin < diff.txt
```

Automation mode prints only the final answer to stdout. Progress and richer interaction belong in the TUI.

## Ratatui implementation notes

- `src/tui.rs` owns terminal state, input handling, inline rendering, and insertion of finalized history above the inline viewport.
- `src/agent.rs` exposes `run_with_events` so frontends can observe steps, streamed model output, and tool calls without the agent writing directly to stdout or stderr.
- The first implementation intentionally keeps the event loop simple and runs one agent turn at a time.

## Next milestones

1. Add async background turn execution so the UI can keep repainting while a model request or tool call is running.
2. Add scrollback controls for long conversations and tool outputs.
3. Split tool output into collapsible detail panels.
4. Add multiline editing and command palette support.
5. Add session persistence and transcript reopening.
6. Add automation-friendly output modes to `rho run`, such as plain text and JSON events.
