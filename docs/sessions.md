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

## Conversation trees

Each saved session is an append-only tree of completed conversation states. Use `/tree` to select any valid turn or compaction state in the current session. Press `up` or `down` to move, type to filter, press `enter` to restore, or press `escape` to cancel. Continuing after you restore an earlier state creates a branch without deleting the path you left. `/info` shows the active leaf ID, node count, and branch count.

Navigation restores conversation and model state only. It does not undo file edits, shell commands, network requests, or any other tool side effects. `/export` renders the active path. The resume picker still shows one row for the whole session, and deleting a session deletes all its branches.

## Compaction and transcript history

Manual and automatic compactions are durable tree states. A compaction node stores the exact model context after summary generation succeeds, while its parent keeps the exact pre-compaction state. The visible transcript keeps the original user, assistant, and tool messages. Selecting the parent lets you continue without that compaction; descendants of the compaction always include it.

Session files use format version 4 for new trees. Rho reads version 1, 2, and 3 files as a single legacy path without rewriting them. The first tree change appends an upgrade record and leaves old bytes unchanged. Older Rho versions cannot resume a session after version 4 records have been appended.

Auto compaction is not a privacy or deletion feature.

## Resetting history

Press `ctrl-r` in the [interactive TUI](/interactive-tui) to reset the conversation. The next message starts a new session file.

For one-shot prompts that do not need an ongoing interactive session, use [automation and CLI](/automation-cli).
