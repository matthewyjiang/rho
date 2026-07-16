---
name: rho-tui-herdr-testing
description: Exploratory Rho interactive TUI smoke tests from inside Herdr. Use when a PTY scenario cannot cover the behavior yet, when investigating a novel visual bug, or when comparing real-pane rendering to the PTY harness. Prefer rho-tui-pty-testing for automated regressions.
compatibility: Requires HERDR_ENV=1, the herdr CLI, and a buildable Rho checkout.
---

# Test the Rho TUI with Herdr

Use Herdr as an exploratory terminal-level harness. Run Rho in a sibling pane, send real input, and inspect the rendered terminal.

For automated, CI-friendly interactive coverage, prefer the PTY harness and the `rho-tui-pty-testing` skill first.

## Prefer PTY first

1. Check whether `cargo test -p rho-coding-agent --test tui_pty` or a named `rho-pty-scenario` already covers the flow.
2. If it does, run the PTY path and stop unless you need live visual confirmation.
3. If it does not, use Herdr to reproduce, then encode a PTY scenario when the behavior is stable.

## Preconditions

- Confirm `HERDR_ENV=1`; otherwise explain that Herdr is required and stop, or fall back to `rho-tui-pty-testing`.
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

### 2. Prefer the deterministic fixture matrix

For provider, streaming, tool, questionnaire, cancellation, failure, steering, compaction, and scrolling flows, launch the debug build with `RHO_TUI_TEST_MODE=matrix`. This replaces the configured provider with a deterministic local fixture and registers a fixture progress tool, so the test needs no credentials or network access:

```bash
RHO_TUI_TEST_MODE=matrix target/debug/rho
```

The fixture is available only in debug builds. Use these exact prompts:

| Prompt | Deterministic behavior |
| --- | --- |
| `fixture stream` | Streams two reasoning chunks and two assistant output chunks with short delays. |
| `fixture tool` | Streams and executes a `write_file` call, then reports exactly one tool result. |
| `fixture questionnaire` | Opens a red/blue questionnaire and reports exactly-once host input delivery. |
| `fixture progress tool` | Runs `tui_fixture_progress`, emits two progress updates, and returns a fixed result. |
| `fixture steering` | Keeps a turn open for two seconds so queued input or steering can be exercised. |
| `fixture steer detail` | Returns a fixed steering acknowledgement. |
| `fixture delay` | Emits partial output and waits 30 seconds for cancellation testing. |
| `fixture input flood` | Emits renderable output events every 5 ms for two seconds to test input fairness under continuous output. |
| `fixture stream failure` | Emits partial output and then returns a permanent provider failure. |
| `fixture bulk one` or `fixture bulk two` | Produces 180 deterministic transcript lines for scrolling tests. |
| `/goal fixture goal retry` | Fails retryably once and then succeeds with the original goal. |

Compaction requests receive a fixed summary. Any other prompt receives `fixture response: <prompt>`.

Use a live provider only when the fixture matrix cannot exercise the behavior under test. When a live run is necessary, use a cheap model such as `gpt-5.4-mini` unless the behavior requires another model. Keep prompts short and use explicit timeouts.

`fixture tool` creates `.rho-tui-fixture-output.txt` in the workspace. Remove that file after the test. Matrix mode replaces model and tool behavior only, so normal session persistence and other application side effects still apply.

### 3. Launch in a sibling pane

```bash
TUI_PANE=$(herdr pane split "$CURRENT_PANE" --direction right --no-focus \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["pane"]["pane_id"])')
herdr pane run "$TUI_PANE" "RHO_TUI_TEST_MODE=matrix target/debug/rho"
herdr wait agent-status "$TUI_PANE" --status idle --timeout 30000
herdr pane read "$TUI_PANE" --source visible --format ansi
```

If the test requires a live provider, omit `RHO_TUI_TEST_MODE=matrix`. If status detection remains unknown, inspect the screen and synchronize on stable startup output rather than using a long fixed sleep.

### 4. Interact like a user

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

### 5. Assert observable behavior

Choose concrete assertions before testing, such as visible text, picker transitions, selection movement, viewport changes, clean shell restoration, or expected agent-state transitions.

Prefer the deterministic fixture matrix over model-dependent tests. For model runs, verify both that status changes to `working` and that the pane shows an actual response or tool call. Text remaining in the composer is not proof of submission.

### 6. Exit and clean up

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

Never leave test panes or processes running. Remove `.rho-tui-fixture-output.txt` if the `fixture tool` flow created it.

## Guidance and report

- Reproduce bugs through the reported user path before fixing them when practical.
- Keep smoke tests focused and avoid timing-sensitive animation assertions.
- Prefer semantic evidence over terminal coordinates; capture ANSI before and after visual interactions.
- After a successful Herdr reproduction of a stable flow, add or extend a PTY scenario when practical.
- Never enter secrets into captured panes.
- Report the flow, assertions, pass/fail/blocked result, focused evidence, and cleanup. Do not dump full logs.
