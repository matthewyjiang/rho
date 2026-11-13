# Upgrading to Rho 3

In Rho 3, your sessions keep running after you quit. If you quit the TUI or close your editor while the agent works, the turn finishes in the background. `rho --resume <id>` picks it up later from any terminal.

This page is for Rho 2 users. The [changelog](/changelog) lists every change.

## Before you upgrade

- Finish or cancel your running workflows. Rho 3 cannot resume a workflow step that Rho 2 left half-finished.
- Copy any session you want to keep opening in Rho 2. Rho 3 opens Rho 2 sessions, but once Rho 3 saves to a session, Rho 2 can no longer open it.

You don't need to change your config first. Rho 3 backs it up and converts it on first start ([Configuration](#configuration)).

## Install

Install Rho 3 as [Installation](/installation) describes, then run `rho` as usual.

Rho 3 runs sessions in a background process, the [daemon](/daemon). Rho starts it for you the first time it needs it. You don't need to set up a service. These commands manage it:

```bash
rho daemon status    # is it running, and what is it doing
rho daemon stop      # stop it; running turns stop and show as interrupted
rho daemon restart   # restart it, for example after an update
rho doctor           # check your setup, the daemon included
```

## What feels different

- Quitting the TUI no longer stops the agent. Rho prints the session id when you quit. Run `rho --resume <id>` to see what happened while you were away.
- Closing your editor, or one of its `rho acp` threads, leaves the session running. Reopen it from the editor or with `rho --resume`.
- `/jobs` in the TUI lists everything Rho is running, across all your projects. You can answer questions, cancel work, and open sessions from there.
- Commands the agent runs use the environment of the terminal or editor you prompted from, so `PATH`, mise tools, and exported variables work as they do in your shell. See [Environment](/daemon#environment).
- Edits to `config.toml` apply to the next session you open. Open sessions keep most of their settings until you reopen them.

New things you can try:

- `rho run --detach "<prompt>"` starts a run in the background and prints its id. `rho attach <id>` follows it ([Running in the daemon](/automation-cli#running-in-the-daemon)).
- `rho run --worktree` gives a session its own git branch and checkout, so parallel sessions in one repository don't overwrite each other ([Running in a worktree](/automation-cli#running-in-a-worktree)).
- `rho mcp serve` lets Claude Code, Codex, or another MCP client start and follow your Rho sessions ([MCP server](/mcp-server)).

## What you need to change

### Use `RHO_HOME` instead of `--config` {#config-and-rho-home}

All sessions use the config in `~/.rho`. To use a different config, set `RHO_HOME` to another directory. That directory gets its own config, sessions, sign-ins, and daemon.

`rho`, `rho run --detach`, `rho login`, and `rho acp` now refuse `--config`. A plain `rho run`, without `--detach`, still accepts it.

### Editor commands

Remove these flags from the command your editor runs for `rho acp`, or it won't start: `--provider`, `--model`, `--config`, `--auth`, `--reasoning`, `--permission-mode`, and `--save`. Pick the model and mode in the editor instead.

Tools now run on the machine where Rho runs. Rho no longer asks the editor to read files, write files, or run commands.

### Configuration

The first time Rho 3 starts, it converts your `config.toml`. Display and keybinding settings move to their own files, and web-search API keys move to your credential store. Rho prints what it moved and saves your original file as `~/.rho/config.toml.2x.bak`. The TUI shows the same summary once. [Upgrading from Rho 2.x](/configuration#upgrading-from-rho-2-x) lists every key.

A few settings need your attention:

- Hooks. If a hook's `tools` list names `edit`, your hooks file fails to load with `tool 'edit' is not in the canonical tool list`. Replace `edit` with `str_replace` and `apply_patch`, and remove `computer`. A hook that checks the payload's schema version must accept version 3 ([Hooks](/hooks)).
- Antigravity agents. In an Antigravity agent definition, replace `view_file` with `read_file` and `create_file` with `write_file`.

You can delete these leftovers. Rho 3 ignores them:

- an `[internal_agents.goal-judge]` table
- `~/.rho/computer-use.toml`
- `~/.rho/computer-use-sessions/`

### Sessions

- Sessions from Rho 1.11 and later open in Rho 3. Older ones fail with `this session was created by rho before 1.11`. Open those with Rho 2.
- Rho 2 and Rho 3 can't use the same session at once. If you try, Rho 3 says the session `is open in another Rho process; close it there first`. Rho 3 lets go of a session one to two minutes after its last activity.

### Scripts

- `rho run --output jsonl` writes a new format, schema version `2`. Events such as `run.completed` are gone. Read the final answer from the `turn_ended` event instead ([Automation output](/automation-cli#automation-output)):

  ```bash
  rho run --output jsonl "summarize this repository" \
    | jq -r 'select(.event.type == "turn_ended") | .event.outcome.text // empty'
  ```

- Subagent `result.json` files name the session and model in `runtime_session_id` and `runtime_model`, not `claude_session_id` and `claude_model`.
- Workflow plan JSON uses `program` and `program_digest` instead of `graph` and `graph_digest`. The error code `legacy_record` is now `unsupported_record`. Top-level task names start with `s0.`, for example `s0.review` ([Workflows](/workflows)).
- Workflows saved before Rho 2.15 no longer open. Delete them from the `/workflow` hub, or inspect them with Rho 2.

## Removed

| Removed | Use instead |
| --- | --- |
| `/goal` | State the outcome you want in your prompt. |
| Computer use: `/computer`, `rho computer`, and the `computer` tool | Nothing. |
| The `hashline` edit format | `str_replace`, the default, or `apply_patch`. Rho 3 switches `edit_tool = "hashline"` to `"str_replace"` for you. |
| The old `rho attach`, which showed a subagent's files | `rho attach <id>`, or `/attach` in the TUI. |
| MCP sampling, where an MCP server asks Rho to run the model | Nothing. The `sampling` setting still loads but does nothing. |

## Things to watch for

- `rho daemon stop` and `rho daemon restart` stop running turns, and the commands and background processes your sessions started.
- The daemon keeps the proxy settings (`HTTPS_PROXY` and similar) and `RHO_LOG` it started with. After you change them, run `rho daemon stop`. The next `rho` command starts the daemon with your shell's values. On Linux, `rho doctor` warns when they differ.
- A `--no-save` session exists only in memory. Quitting the TUI cancels its turn, and a daemon restart loses it.

## Updating Rho 3 later

When you update Rho, with `rho update`, the install script, Cargo, or pacman, the daemon switches to the new version once nothing is running.

- With mise, run `rho daemon restart` after the update, once your work is done.
- On Windows, `rho update` asks you to stop the daemon first. Run `rho daemon stop` once your work is done, then the update command it printed.

`rho uninstall` also stops the daemon and interrupts running turns ([The daemon and services](/installation#the-daemon-and-services)).

## Going back to Rho 2

1. Stop the daemon:

   ```bash
   rho daemon stop
   ```

2. Install the last Rho 2 release. With the install script, set `RHO_VERSION` to it, for example:

   ```bash
   curl -fsSL https://matthewyjiang.github.io/rho/install.sh | RHO_VERSION=2.34.0 sh
   ```

   With Cargo, run `cargo install rho-coding-agent --version 2.34.0`.

3. Restore your Rho 2 config:

   ```bash
   cp ~/.rho/config.toml.2x.bak ~/.rho/config.toml
   ```

   This also brings back your web-search keys, which the backup still holds.

Rho 2 cannot open sessions that Rho 3 saved to. Use the copies you made before upgrading.

If you build on Rho's Rust libraries, the [changelog](/changelog) lists their API changes.
