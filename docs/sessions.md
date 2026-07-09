# Sessions

Rho persists interactive conversation history so you can resume work later.

## Storage location

Sessions persist automatically under:

```text
~/.rho/sessions/<workspace-key>/
```

`<workspace-key>` contains a readable encoding of the absolute working directory plus a stable hash to avoid path collisions. Rho uses the current directory as its [workspace](/tools-workspace).

## Creating a session

Starting `rho` opens the [interactive TUI](/interactive-tui). Rho creates a new session file only after you send the first message.

## Resuming a session

To resume an existing session for the current workspace, pass its UUID or UUID prefix with `--resume` or `-R`:

```bash
rho --resume <session-uuid>
rho -R <session-uuid-prefix>
```

You can also omit the ID to open an interactive picker for saved sessions in the current workspace:

```bash
rho --resume
rho -R
```

Inside the TUI, use `/resume [id]` to switch sessions. With no ID, `/resume` opens the same saved-session picker.

After you send at least one message, Rho restores your shell view on exit and prints a short saved-session summary plus a resume command that you can paste later.

## Auto compaction and transcript history

When [auto compaction](/configuration#auto-compaction) is enabled, Rho can append a replacement-history entry that summarizes older model context for future requests and resume. The original message entries remain in the session file for transcript reconstruction. Auto compaction is not a privacy or deletion feature.

## Resetting history

Press `ctrl-r` in the [interactive TUI](/interactive-tui) to reset the conversation. The next message starts a new session file.

For one-shot prompts that do not need an ongoing interactive session, use [automation and CLI](/automation-cli).
