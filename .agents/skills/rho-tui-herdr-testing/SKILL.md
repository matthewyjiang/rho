---
name: rho-tui-herdr-testing
description: Test Rho's interactive TUI end to end from inside Herdr. Use when changing or validating TUI rendering, input, keybindings, pickers, scrolling, commands, lifecycle state, startup, shutdown, or terminal restoration. Drive a local Rho build in a sibling pane, inspect rendered output, and clean up afterward.
compatibility: Requires HERDR_ENV=1, the herdr CLI, and a buildable Rho checkout.
---

# Test the Rho TUI with Herdr

Use Herdr as a terminal-level test harness. Run Rho in a sibling pane, send real input, and inspect the rendered terminal. This catches integration issues that unit tests miss.

## Preconditions

- Confirm `HERDR_ENV=1`; otherwise explain that Herdr is required and stop.
- Run from the Rho repository root and preserve unrelated changes.
- Use `herdr pane list` to identify the focused pane. Never replace the current agent pane with the test TUI.

## Important Herdr features

- `pane split --no-focus` creates an isolated terminal while keeping this agent usable.
- `pane run` sends text and a real Enter atomically. Prefer it for shell commands and submitted prompts.
- `pane send-text` types without submitting; `pane send-keys` sends keys such as `Enter`, `Esc`, `Tab`, `Up`, `Down`, `PageUp`, `Ctrl-g`, and `Ctrl-c`.
- `pane read --source visible --format ansi` captures the rendered viewport. Use `recent-unwrapped` for searchable transcript text.
- `wait output` synchronizes on future text; `wait agent-status` synchronizes on Rho's `idle`, `working`, `blocked`, or `done` state. Status alone does not prove a prompt was submitted, so also read the pane.
- Pane IDs can compact after closures. Parse IDs from command responses and re-list panes when the layout changes.

## Workflow

### 1. Build

Capture verbose output in a temporary log:

```bash
BUILD_LOG=$(mktemp /tmp/rho-tui-build.XXXXXX.log)
cargo build >"$BUILD_LOG" 2>&1
```

On failure, inspect focused excerpts and stop rather than launching an obsolete binary.

### 2. Launch in a sibling pane

```bash
TUI_PANE=$(herdr pane split "$CURRENT_PANE" --direction right --no-focus \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["pane"]["pane_id"])')
herdr pane run "$TUI_PANE" "target/debug/rho"
herdr wait agent-status "$TUI_PANE" --status idle --timeout 30000
herdr pane read "$TUI_PANE" --source visible --format ansi
```

If status detection remains unknown, inspect the screen and synchronize on stable startup output rather than using a long fixed sleep.

### 3. Interact like a user

```bash
# Submit a prompt
herdr pane run "$TUI_PANE" "summarize this repository in one sentence"

# Type and navigate without immediate submission
herdr pane send-text "$TUI_PANE" "/mod"
herdr pane send-keys "$TUI_PANE" Tab
herdr pane send-keys "$TUI_PANE" Enter
herdr pane send-keys "$TUI_PANE" Esc
```

Useful Rho controls:

- `/` opens the command palette; `@` opens file completion.
- `Up` and `Down` select, `Tab` completes, `Enter` confirms, and `Esc` cancels or interrupts.
- `PageUp` and `PageDown` scroll; `Ctrl-g` returns to the bottom.
- `Ctrl-r` resets history. `Ctrl-c` clears input, then exits when pressed again. Since `pane send-keys` does not currently accept `Ctrl-c`, send its literal control byte from Bash or Zsh:

  ```bash
  herdr pane send-text "$TUI_PANE" $'\003'
  ```

  Send the command twice to exercise Rho's clear-then-exit behavior.
- `Enter` queues a prompt during a run; `Alt-Up` restores the latest queued prompt to the composer.

Rho owns its transcript viewport, so use these controls instead of terminal scrollback.

### 4. Assert observable behavior

Choose concrete assertions before testing, such as visible text, picker transitions, selection movement, viewport changes, clean shell restoration, or expected agent-state transitions.

For model runs, verify both that status changes to `working` and that the pane shows an actual response or tool call. Text remaining in the composer is not proof of submission.

Prefer local UI transitions over model-dependent tests. When prompts are required, use a cheap model such as `gpt-5.4-mini`; use another model only for behavior that specifically requires it. Keep prompts short and use explicit timeouts.

### 5. Exit and clean up

Exercise the user-facing exit path first:

```bash
herdr pane run "$TUI_PANE" "/exit"
```

Verify the shell returns and the alternate screen is restored. Use `Esc` or send a literal Ctrl-c if stuck, then close the temporary pane:

```bash
herdr pane send-keys "$TUI_PANE" Esc
herdr pane send-text "$TUI_PANE" $'\003'
herdr pane close "$TUI_PANE"
```

Never leave test panes or processes running.

## Guidance and report

- Reproduce bugs through the reported user path before fixing them when practical.
- Keep smoke tests focused and avoid timing-sensitive animation assertions.
- Prefer semantic evidence over terminal coordinates; capture ANSI before and after visual interactions.
- Never enter secrets into captured panes.
- Report the flow, assertions, pass/fail/blocked result, focused evidence, and cleanup. Do not dump full logs.
