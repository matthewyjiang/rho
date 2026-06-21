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

After you send at least one message, Rho prints a resume command on exit that you can paste later.

## Resetting history

Press `ctrl-r` in the [interactive TUI](/interactive-tui) to reset the conversation. The next message starts a new session file.

For one-shot prompts that do not need an ongoing interactive session, use [automation and CLI](/automation-cli).
