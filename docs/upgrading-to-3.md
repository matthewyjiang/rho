# Upgrading to Rho 3

Rho 3 runs your sessions in a background process, the [daemon](/daemon). Quitting the TUI or closing your editor no longer stops the agent: the turn keeps running, and you can pick it up again later from any terminal or editor.

This page covers how to upgrade from Rho 2.x, what is new, what was removed and what to use instead, and the known limits. The [changelog](/changelog) lists every change.

## Before you upgrade

- **Finish or cancel running workflows.** Rho 3 cannot recover a workflow step that Rho 2.x left half-finished.
- **Copy any session you may want to reopen in Rho 2.x.** Rho 3 opens your 2.x sessions, but after Rho 3 saves to a session, Rho 2.x can no longer open it.

Your config needs no preparation. Rho 3 backs it up before it changes anything ([Configuration](#configuration)).

## Install

Install Rho 3 as [Installation](/installation) describes, then run `rho` as before. The daemon starts by itself the first time a command needs it, on Linux, macOS, and Windows. It needs no setup and no service.

You rarely need these commands, but they are there:

| Command | What it does |
| --- | --- |
| `rho daemon status` | Show whether the daemon is running and what it is doing. |
| `rho daemon stop` | Stop the daemon. Turns that are running stop and show as interrupted. |
| `rho daemon restart` | Restart the daemon, for example to switch it to a version you just installed. |
| `rho doctor` | Check your setup, the daemon included. |

When you update Rho, a running daemon switches to the new version by itself once nothing is running. If you installed Rho with a tool that puts each version in its own directory, such as mise or Homebrew, the daemon stays on the old version. Run `rho daemon restart` after you update ([Upgrading in place](/daemon#upgrading-in-place)).

## What's new

- **Sessions outlive the TUI.** Quit while the agent is working and the turn keeps going. `rho --resume <id>` reopens the session and shows what happened while you were away. The id is printed when you quit.
- **`/jobs`** shows everything the daemon is running, in every project. From there you can answer questions, send messages, cancel work, and open a session ([Interactive TUI](/interactive-tui)).
- **Run in the background from a script.** `rho run --detach "<prompt>"` starts a run, prints its id, and exits. `rho attach <id>` follows it, answers its questions, and exits when the turn ends. It also follows a subagent ([Running in the daemon](/automation-cli#running-in-the-daemon)).
- **Editor sessions keep running.** Closing an editor that runs `rho acp`, or one of its threads, leaves the session running. Open it again from the editor or with `rho --resume` ([Agent Client Protocol](/integrations/acp)). In T3 Code with `--t3code`, a background process or workflow that finishes after a prompt has ended wakes the idle thread with its result ([T3 Code](/integrations/acp/t3-code#wake-on-background-results)).
- **Sessions can work in their own git worktree.** `rho run --worktree`, `Tab` when you start a session from `/jobs`, or the `worktree` option of the MCP server's `new` gives a session a new branch checked out under `~/.rho/worktrees/`, so sessions running at once in one repository don't overwrite each other's edits. Rho never deletes these worktrees or their branches: remove them yourself with `git worktree remove` and `git branch -D` once you've kept what you want ([Running in a worktree](/automation-cli#running-in-a-worktree)).
- **Other agents can drive Rho.** Add `rho mcp serve` as an MCP server in Claude Code, Codex, or another Rho, and that agent can list your Rho sessions, start new ones, and follow them to their answer. Approvals and questions still wait for you ([Serving Rho to other agents](/integrations/mcp#serving-rho-to-other-agents)).
- **Commands see your shell's environment.** Each prompt carries the environment of the terminal or editor it came from, so `PATH`, `mise` tools, and exported variables work as they do in your shell ([Environment](/daemon#environment)).
- **The model knows when you wrote.** Each message you send reaches the model with the date and time you sent it. Your transcript shows the message as you typed it.
- **Keep-warm works without the TUI.** With `behavior.keep_cache_warm` on, the daemon keeps the provider's prompt cache warm whether or not a TUI is open.

## What changes

### `--config` is replaced by `RHO_HOME` {#config-and-rho-home}

Every session the daemon runs uses the config in your Rho home (`~/.rho`). To use a different config, point `RHO_HOME` at another directory. That directory gets its own daemon, config, sessions, and sign-ins.

These commands refuse `--config`:

| Command | Message starts with |
| --- | --- |
| `rho` | `--config is not supported in a daemon session` |
| `rho run --detach` | `rho run --detach does not take --config` |
| `rho login` | `rho login does not take --config` |
| `rho acp` | `rho acp does not take --config` |

`rho run` without `--detach` runs in your terminal and still accepts `--config`.

### Editors

Remove these flags from the command your editor runs: `--provider`, `--model`, `--config`, `--auth`, `--reasoning`, `--permission-mode`, and `--save`. `rho acp` refuses to start with them. Choose the model and mode in the editor instead.

Tools now run on the machine where the daemon runs. Rho no longer asks the editor to read and write files or to run terminal commands for it.

### Configuration

The first time Rho 3 starts, it rewrites your 2.x `config.toml` into the 3.0 layout. Display and keybinding settings move to their own files, and web-search API keys move to your credential store. Rho prints what it moved and saves the original as `config.toml.2x.bak`. [Upgrading from Rho 2.x](/configuration#upgrading-from-rho-2-x) lists every key.

Some settings need you:

- **Hooks.** A hook whose `tools` list names `edit` stops your hooks file from loading, with the error `tool 'edit' is not in the canonical tool list`. Replace `edit` with `str_replace` and `apply_patch`. Remove `computer` too. A hook program that checks the payload's schema version must accept version 3 ([Hooks](/hooks)).
- **Antigravity agents.** In an Antigravity agent definition, replace `view_file` with `read_file` and `create_file` with `write_file`.
- **Leftovers you can delete:** an `[internal_agents.goal-judge]` table, `~/.rho/computer-use.toml`, and `~/.rho/computer-use-sessions/`. Rho ignores them.

`edit_tool = "hashline"` changes to `"str_replace"` by itself.

### Sessions

- **Sessions from Rho 1.11 and later** open in Rho 3. Once Rho 3 saves to one, Rho 2.x cannot open it.
- **Sessions from before Rho 1.11** do not open, and show `this session was created by rho before 1.11 (session format v2) and can't be opened in rho 3; open it with rho 2.x`. Rho 2.x still opens them.
- **One program at a time.** Rho 2.x and the Rho 3 daemon cannot use the same session at the same time. Rho 3 then reports that the session `is open in another Rho process; close it there first`. The daemon lets go of a session one to two minutes after its last activity ([One process per session](/sessions#one-process-per-session)).

### Scripts

- **Subagent `result.json`.** The session id and model are in `runtime_session_id` and `runtime_model`, not `claude_session_id` and `claude_model`.
- **Workflow JSON.** Plans report `program` and `program_digest` instead of `graph` and `graph_digest`. The error code `legacy_record` is now `unsupported_record`. Top-level task names gain an `s0.` prefix, for example `s0.review` ([Workflows](/workflows)).
- **Workflows saved before Rho 2.15** no longer open. Delete them from the `/workflow` hub, or inspect them with Rho 2.x.

## Removed

| Removed | Use instead |
| --- | --- |
| Running the TUI without the daemon, and `RHO_TUI_DAEMON` | Nothing to do. The TUI always uses the daemon, and Rho ignores the variable. |
| `/goal` | State the outcome you want in your prompt. `/goal` kept prompting the agent until a checker agreed it was done. Current models no longer need that. |
| Computer use: `/computer`, `rho computer`, and the `computer` tool | None. The daemon may not have access to your desktop. |
| `--config` for daemon sessions | `RHO_HOME` ([above](#config-and-rho-home)). |
| The `hashline` edit format | `str_replace` (the default) or `apply_patch` ([Edit formats](/tools-workspace/edit-format)). |
| The old `rho attach`, which showed a subagent's files | `rho attach <id>`, or `/attach` in the TUI. |
| Answering MCP servers that ask Rho to run the model (sampling) | None. |
| Opening sessions from before Rho 1.11 | Rho 2.x. |
| Opening workflows saved before Rho 2.15 | Rho 2.x. |

## Known limits

- **MCP sampling.** Rho does not answer an MCP server that asks it to run the model. The `sampling` setting still loads but does nothing ([Model Context Protocol](/integrations/mcp)).
- **Stopping the daemon stops your work.** `rho daemon stop` and `rho daemon restart` interrupt running turns and end the commands and background processes your sessions started.
- **The daemon keeps its own settings.** Network settings such as `HTTPS_PROXY`, and `RHO_LOG`, come from the environment the daemon started with. After you change them, run `rho daemon stop`, and the next `rho` command starts the daemon with your shell's settings. On Linux, `rho doctor` warns when they differ from your shell's.
- **Unsaved sessions are lost on restart.** A session started with `--no-save` lives only in the daemon's memory. Quitting the TUI cancels its turn, and a daemon restart loses it.

If you build on Rho's Rust libraries, the [changelog](/changelog) lists their API changes.
