---
name: rho-tui-pty-testing
description: Test Rho's interactive TUI with the deterministic PTY harness. Use when changing or validating TUI rendering, input, keybindings, pickers, scrolling, commands, lifecycle state, startup, shutdown, resize, paste, cancellation, or terminal restoration. Prefer this over Herdr for automated and CI-friendly checks.
compatibility: Requires a buildable Rho checkout. Unix PTY support is required for scenario runs.
---

# Test the Rho TUI with the PTY harness

Use the `rho-tui-pty` crate to spawn a compiled Rho binary in a pseudo-terminal, inject input, reconstruct the visible screen, and assert user-visible behavior. This is the primary automated interactive TUI path.

## When to use this vs Herdr

| Need | Tool |
| --- | --- |
| Known flows, regressions, CI | PTY harness (this skill) |
| Novel bug, live exploratory inspection | Herdr (`rho-tui-herdr-testing`) |
| Compare virtual screen vs real pane | Herdr, then encode as a PTY scenario |

## Preconditions

- Run from the Rho repository root.
- Prefer debug builds so `RHO_TUI_TEST_MODE=matrix` is available.
- Capture verbose command output in temp logs.

## Build

```bash
BUILD_LOG=$(mktemp /tmp/rho-tui-build.XXXXXX.log)
cargo build -p rho-coding-agent >"$BUILD_LOG" 2>&1
```

On failure, inspect focused excerpts and stop.

## Run the smoke suite

```bash
TEST_LOG=$(mktemp /tmp/rho-pty-smoke.XXXXXX.log)
cargo test -p rho-coding-agent --test tui_pty >"$TEST_LOG" 2>&1
```

Smoke scenarios:

- `startup_stream_exit`
- `cancel_and_resubmit`
- `resize_during_stream`
- `scroll_during_stream`
- `terminal_restoration`

## Run one scenario

```bash
cargo run -p rho-tui-pty --bin rho-pty-scenario -- --list
cargo run -p rho-tui-pty --bin rho-pty-scenario -- \
  --bin target/debug/rho \
  --artifacts /tmp/rho-pty-artifacts \
  startup_stream_exit
```

Useful flags:

- `--smoke` runs the CI subset
- `--timing` prints wait latency samples and p50/p95/p99
- `--artifacts <dir>` writes failure bundles
- `--bin <path>` selects the Rho binary

## Fixture matrix prompts

Scenarios use `RHO_TUI_TEST_MODE=matrix` automatically. Exact prompts:

| Prompt | Deterministic behavior |
| --- | --- |
| `fixture stream` | Streams reasoning and assistant chunks |
| `fixture tool` | Writes `.rho-tui-fixture-output.txt` via `write_file` |
| `fixture questionnaire` | Red/blue questionnaire |
| `fixture progress tool` | Progress updates then fixed result |
| `fixture delay` | Partial output, long wait for cancellation |
| `fixture bulk one` / `fixture bulk two` | Long transcript for scrolling |
| other text | `fixture response: <prompt>` |

## Writing or extending scenarios

1. Add steps in `crates/rho-tui-pty/src/scenarios.rs`.
2. Assert user-visible text from the reconstructed screen, not internal status strings.
3. Use bounded waits (`wait_for_text`, `wait_for_quiet`, `wait_for_exit`), never fixed-only sleeps as the sole sync.
4. Type through harness helpers so paste-burst detection does not swallow Enter.
5. Keep scenarios independent of the developer HOME, credentials, and host terminal markers.
6. Mark stable CI cases with `smoke: true`.

## Failure artifacts

On failure, inspect:

- `raw.pty` - full PTY byte stream
- `screen.txt` - reconstructed visible screen
- `actions.log` - timestamped action timeline
- `report.json` - structured summary with redacted env

## Harness self-tests

```bash
cargo test -p rho-tui-pty
```

These cover timeouts, child exit codes, resize, kill-on-drop, and screen parsing without launching Rho.

## Guidance and report

- Prefer semantic screen text over raw ANSI snapshots.
- Do not require pixel-perfect terminal emulator parity.
- Keep performance timing advisory unless a metric is known-stable.
- Report the scenario ids, pass/fail, artifact paths, and any skipped platform capability.
