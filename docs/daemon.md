# Daemon

Rho runs your sessions in a background process called the daemon. The TUI, your editor's `rho acp`, and other clients connect to it. When a client quits, the daemon keeps the session and its turn running, and any client can pick it up later.

There is one daemon per user and Rho home (`~/.rho`, or `$RHO_HOME`). Rho starts it the first time a command needs it, so you rarely manage it yourself.

## Commands

| Command | What it does |
| --- | --- |
| `rho daemon status` | Show whether the daemon runs, its version, its jobs, and a pending update. Add `--json` for JSON. |
| `rho daemon start` | Start the daemon if none runs. Safe to run again. |
| `rho daemon stop` | Stop the daemon. Running turns stop and show as `interrupted`. |
| `rho daemon restart` | Restart the daemon, for example after an update. Running turns stop and show as `interrupted`. |
| `rho daemon` | Run the daemon in the foreground. Same as `rho daemon run`. |
| `rho attach <id>` | Follow a session's turn from the terminal ([Attaching](/automation-cli#attaching-to-a-daemon-session)). |
| `rho worktrees prune [--dry-run]` | Remove session worktrees that are clean and merged ([Sessions in worktrees](#sessions-in-worktrees)). |
| `rho mcp serve` | Let another agent start and follow your sessions ([MCP server](/mcp-server)). |

`rho daemon status`, `rho daemon stop`, and `rho attach` never start a daemon. With none running, they print `rho daemon is not running` and exit with status 1.

Stopping the daemon also ends the commands and background processes your sessions started. If the daemon crashes or you kill it with `SIGKILL`, those processes keep running.

## The TUI {#daemon-backed-tui}

`rho` opens the TUI as a client of the daemon. Things that work differently from Rho 2:

- Quitting the TUI leaves a running turn going. The exit message shows `rho --resume <id>`. Run it later to see the reply, even one that finished while the TUI was closed.
- A new session reaches the daemon with its first prompt. If you open `rho` and quit without typing, nothing is left behind.
- `/model`, `/permissions`, and the other settings commands change the current session. A change you make during a turn applies when the turn ends.
- A workflow or background process that a turn started keeps running after the turn ends. When it finishes, the daemon starts a new turn that gives the model the result, whether or not a TUI is open.
- `/jobs` lists everything the daemon runs, in every project. From there you can answer questions, send messages, cancel work, and open sessions ([Interactive TUI](/interactive-tui)).
- `/login` opens the browser on the machine where the TUI runs. Sessions use the new sign-in from their next prompt.
- You can't delete a session from a picker while the daemon has it loaded.
- After `rho daemon restart`, or an update the daemon picks up on its own, the TUI reconnects by itself. If the daemon stops instead, after `rho daemon stop` or a mise or Windows update, reopen the session with `rho --resume <id>`.

Launch flags work as follows:

- `--provider`, `--model`, `--auth`, `--reasoning`, and `--permission-mode` apply to a new session at its first prompt, and to a resumed session once it opens. The bare `--resume` picker refuses them.
- `--agent` with `--resume` or `--continue` must name the agent the session was saved as.
- `--config` is refused. Every session uses the config in your Rho home. Set `RHO_HOME` to use another one.
- A `--no-save` session exists only in the daemon's memory. Quitting the TUI cancels its turn, and a daemon restart loses it.

With [`behavior.keep_cache_warm`](/configuration) on, the daemon keeps the provider's prompt cache warm whether or not a TUI is open.

## Sessions and jobs

The daemon tracks each piece of work as a job: a session's turns, a subagent run, or a workflow run. `/jobs` and `rho daemon status` list them. A job is in one of these states:

| State | Meaning |
| --- | --- |
| `idle` | Opened, no turn started yet. |
| `queued` | Waiting for a free agent slot ([Subagents](#subagents)). |
| `running` | A turn is under way. |
| `waiting_for_input` | The turn waits for an approval or an answer to a question. |
| `completed` | The last turn finished. |
| `failed` | The last turn failed. The job shows why. |
| `cancelled` | Someone cancelled the last turn. |
| `timed_out` | The last turn hit its time limit, such as `rho run --detach --timeout`. |
| `interrupted` | The daemon stopped before the last turn ended. |

A turn that waits for an approval or an answer waits until someone answers or cancels it. Rho never denies it because no client is attached. Any connected client can answer, and the first answer wins. The exception is a question with fallback answers when you set [`[questionnaire] timeout_seconds`](/configuration#questionnaire-timeout). Rho answers it with the fallback once the timeout passes.

To follow a session from a terminal, use [`rho run --detach`](/automation-cli#running-in-the-daemon) and [`rho attach`](/automation-cli#attaching-to-a-daemon-session). Editors connect through [`rho acp`](/integrations/acp). Closing an editor thread leaves its session in the daemon.

Prompting or attaching to a saved session reopens it in its own workspace, whatever directory you run from. A reopen fails when:

- another program, such as a foreground `rho run` or Rho 2, has the session open
- a subagent or workflow node owns the session, since only the session that started it can continue it
- the agent the session was saved as has changed or no longer exists

### Sessions in worktrees

A session can work in its own git worktree, so several sessions in one repository don't overwrite each other's edits. Ask for one with [`rho run --worktree`](/automation-cli#running-in-a-worktree), with `Tab` when you start a session from `/jobs`, or with the `worktree` option of the [MCP server](/mcp-server)'s `new` tool.

The daemon creates a branch `rho/<repository>-<id>` from your current `HEAD` and checks it out at `~/.rho/worktrees/<repository>-<id>`. The session's commands, edits, and project instructions use the worktree. The workspace must be inside a git repository.

`rho daemon status` lists the worktrees of the daemon's sessions:

```text
  worktrees    rho/api-3fa2c1d0 /home/me/.rho/worktrees/api-3fa2c1d0
               rho/api-91bd22aa /home/me/.rho/worktrees/api-91bd22aa (stale)
```

The daemon removes a worktree once its session has sat unused for a day, the worktree has no uncommitted changes, and your checkout already has its commits. It keeps the branch, and it recreates the worktree if you reopen the session. Worktrees it can't remove are marked stale after 14 days. [Cleaning up worktrees](/automation-cli#cleaning-up-worktrees) covers the rules, `rho worktrees prune`, and how to remove a worktree yourself.

To open a worktree session from a TUI started anywhere, select it in `/jobs` and press `Enter`. The TUI moves into the worktree. You can also run `rho attach <id>`.

### Subagents {#subagents}

Subagents run in the daemon as jobs of their own. Follow one with `rho attach <job id>`. Its questions go to the session that started it.

All subagents, workflow agent nodes, and sessions started by agents share one budget, `agent_concurrency` in your config, 10 by default, across all your projects. Work beyond the budget waits as `queued` and shows its place in line. Claude Code subagents also have their own limit of 2, set with `RHO_CLAUDE_AGENT_CONCURRENCY` in the daemon's environment. A change to `agent_concurrency` applies at once.

### Sessions other sessions start {#sessions-other-sessions-start}

An agent can start other sessions with its `sessions` tool. Each started session gets no more permissions than the turn that started it, whoever prompts it later. Its turns use the same budget as subagents.

A turn that waits for a person, or for sessions it started, gives its slot back while it waits. This prevents deadlocks, but it means the number of running agents can go somewhat above `agent_concurrency`. [MCP server limits](/mcp-server#limits) cap how many sessions an agent can start.

### Workflow runs

A workflow run started from a session runs in the daemon and shows in `/jobs`. It follows its permission mode and never stops to ask you for approval. A node's `timeout_seconds` includes time spent waiting for an agent slot.

If the daemon stops during a run, the run shows as `interrupted`. The daemon doesn't resume it on its own. Resume it with the session's `workflow` tool, or with `rho workflow resume --recover-uncertain` once you have checked that no process the interrupted node started is still running.

### Unloading idle sessions

After a session sits idle for 60 seconds, the daemon unloads it. It stops the session's MCP servers and releases the session file, so another program, such as a foreground `rho run`, can open it. The next prompt or attach reopens it within moments.

A session is not idle while a turn, subagent, workflow run, or background process of its own is running, or while a client follows it.

A session that never saved anything is discarded instead of unloaded, and can't be reopened.

A reopened session reads the current `config.toml`. It keeps the model, reasoning level, fast mode, and permission mode you chose for it. Approvals you granted for the session reset.

To change the idle period, run the daemon yourself with `rho daemon run --unload-idle-after 10m`.

## Environment

Commands a session runs use the environment of the terminal or editor you prompted from, not the daemon's. `PATH`, mise shims, `SSH_AUTH_SOCK`, and an active virtualenv work as they do in your shell. Each prompt sends the current environment, so after `mise use` or `export FOO=bar`, the next prompt's commands see the change.

- A background process keeps the environment it started with.
- If a prompt brings a different environment, Rho asks again about commands you allowed for the session.
- Rho removes provider API keys, such as `OPENAI_API_KEY`, from the environment of commands, as in Rho 2.
- MCP servers and OpenAPI sources use the environment from when the session opened. Hooks use the environment of the prompt that runs them.
- [Claude Code](/subagents/claude-cli), [Cursor](/subagents/cursor), and [Antigravity](/subagents/antigravity) subagents get the full environment, API keys included, since they sign in on their own.
- A workflow run keeps the environment it started with. If a later resume finds a different interpreter for a script on its `PATH`, the run fails with `frozen filesystem identity drift`.

Project trust variables (`RHO_TRUST_PROJECT_HOOKS`, `RHO_TRUST_PROJECT_AGENTS`, `RHO_TRUST_PROJECT_PLUGINS`, and `RHO_TRUST_PROJECT_OPENAPI`) come from the terminal that opens the session. A prompt from a terminal with different trust settings is refused, and the error names the variables that differ. Open a new session from that terminal, or set the same variables.

### The daemon's own environment {#starting-automatically}

When a client starts the daemon, the daemon keeps only a few variables from that client's environment:

| Kind | Variables |
| --- | --- |
| Paths and user | `HOME`, `USER`, `LOGNAME`, `PATH`, `TMPDIR`, `XDG_RUNTIME_DIR`, `XDG_CACHE_HOME`, `XDG_CONFIG_HOME`, `GIT_CONFIG_GLOBAL`, `LD_LIBRARY_PATH`, `DYLD_LIBRARY_PATH`, `RHO_HOME` |
| Locale | `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ` |
| Network | `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY` and their lowercase forms, `SSL_CERT_FILE`, `SSL_CERT_DIR`, `RHO_SSRF_ALLOW_RANGES` |
| Daemon settings | `RHO_LOG`, `RHO_CLAUDE_AGENT_CONCURRENCY`, `RHO_MODELS_PATH`, `RUST_BACKTRACE` |

The daemon reads these once, when it starts. After you change a proxy setting or `RHO_LOG`, run `rho daemon stop`. The next `rho` command starts a daemon with your shell's values. On Linux, `rho doctor` warns when they differ.

`RHO_SSRF_ALLOW_RANGES` applies to every session's web tools. If you run `rho daemon run` yourself, or from a service manager, the daemon keeps its whole environment.

## Credentials

A session signs in with the API keys in the environment of the client that opens it, such as `OPENAI_API_KEY` or `GITHUB_TOKEN`. A key there wins over a saved one. Without one, the session uses your [credential store](/authentication-and-models#where-credentials-live) and asks you to sign in if it finds nothing.

- A session keeps the keys it opened with while it stays loaded. A prompt from a terminal with a different key still runs on the first one.
- An API key in the daemon's own environment signs in no session. Put it in your shell, or save it with `rho login`.
- `rho login`, `/login`, and the editor's sign-in run through the daemon. Running sessions pick up the change at their next prompt.
- You can't replace or remove a key with `rho login` or `/logout` while your shell sets it. Unset it first.
- `RHO_CREDENTIAL_STORE` in your shell picks the store for the sessions you open.
- For the [Ollama Cloud device key](/providers/ollama-cloud#setup-with-a-device-key), set `OLLAMA_DEVICE_KEY_DIR` to an absolute path. Without it, Rho uses `~/.ollama`.

On Linux, the daemon reaches the system keyring through your desktop session. A daemon started from SSH, or on a machine without a desktop, can't reach it. Rho then refuses to save credentials to the keyring rather than fall back to a file. `rho login` offers the file store instead. You can also switch with:

```bash
rho credential-store set file
```

`rho daemon status` and `rho doctor` show which store the daemon uses and why it can't reach it.

## Configuration changes

You don't need to restart the daemon after you edit `config.toml`. Each new session reads the file as it is when it opens, including one you start with `/new`. A session that is already open keeps most settings until it reopens.

Some settings reach open sessions sooner:

- `agent_concurrency` applies at once.
- Advisor, compaction, codemode, and session-title settings apply at each session's next turn.
- Custom providers and `[providers.ollama]` apply at the next turn. A session whose provider you removed fails each turn until you add it back or switch models.

If `config.toml` stops loading, open sessions keep the last good settings. A new session fails with an error that names each problem:

```text
could not open session: invalid config file /home/you/.rho/config.toml:
`behavior.edit_tool`: must be one of auto, str_replace, apply_patch; got "sed"
```

Fix the file and open the session again.

The daemon reads these from its own environment only when it starts: `RHO_LOG`, `RHO_HOME`, `XDG_RUNTIME_DIR`, `RHO_CLAUDE_AGENT_CONCURRENCY`, `RHO_CREDENTIAL_STORE`, and `--unload-idle-after`. Run `rho daemon stop` to apply a change. `RHO_CREDENTIAL_STORE` in your shell still picks the store for the sessions you open ([Credentials](#credentials)).

## Updates {#upgrading-in-place}

When you update Rho, the running daemon notices the new executable and switches to it once nothing is running. It doesn't interrupt your work. While it waits, `rho daemon status` shows `upgrade  pending to <version> (<N> in progress)`. Running turns, subagents, workflow runs, background processes, logins in progress, and questions waiting for an answer all keep it waiting.

`rho daemon restart` switches right away and interrupts running turns.

If you install with mise or Homebrew, the new version lands at a different path, and the daemon can't see it. Run `rho daemon restart` from the new version once your work is done.

## Versions

If your `rho` and the daemon are different versions, here is what happens:

| Situation | What happens |
| --- | --- |
| Compatible versions | `rho` connects. |
| Same protocol, different build | `rho` connects, and the TUI shows a notice. Run `rho daemon restart` to replace the daemon. |
| Older daemon with no work and no other clients | `rho` replaces it. |
| Older daemon in use | `rho` fails and tells you to run `rho daemon restart`, with how many jobs and clients it would interrupt. Run it, or wait until the daemon is free. |
| Newer daemon | `rho` fails and tells you to update. A TUI left open across an update hits this. Reopen the session with `rho --resume <id>`. |

`rho daemon stop` and `rho daemon restart` work with any daemon version.

## Checking the daemon

`rho doctor` and `/doctor` report on the daemon without starting one. `rho doctor --json` includes the same checks.

| Row | Warns or fails when |
| --- | --- |
| `Daemon` | The daemon's version is different from yours, or can't work with it ([Versions](#versions)). |
| `Daemon socket` | The socket or its directory is open to other users. |
| `Service` | A `rho-mcp.service` systemd unit is saved but not enabled ([Running as a service](#running-as-a-service)). |
| `Daemon environment` | The daemon's proxy settings or `RHO_LOG` differ from your shell's. Linux only. |
| `Daemon credential store` | The daemon can't reach its credential store ([Credentials](#credentials)). |

## Socket and permissions

Only your user can connect to the daemon. It listens on no network port.

On Linux and macOS, the daemon listens on a Unix socket, `daemon.sock`, under `$XDG_RUNTIME_DIR/rho/` or, without `XDG_RUNTIME_DIR`, under `~/.rho/run/`. Rho refuses to use the socket directory if other users can access it. Socket paths are limited to 107 bytes on Linux and 103 on macOS. If yours is longer, Rho fails with an error. Point `RHO_HOME` or `XDG_RUNTIME_DIR` at a shorter directory.

On Windows, the daemon listens on a named pipe that only your user can open. `Ctrl+C` stops a `rho daemon run` started in a console.

## Logs

The daemon writes its log to `~/.rho/logs/daemon.log`. If you run `rho daemon run` itself as a systemd service, it logs to the journal instead. Set `RHO_LOG` to change the level, for example `RHO_LOG=rho=debug`. The default is `info`.

The log can contain provider error messages that echo credentials, so keep it private. Rho creates the file readable only by you. The journal doesn't get that protection.

## Running as a service

The daemon doesn't need a service. Rho starts it on demand. To have it running before anything connects, for example in a script, run `rho daemon start`.

You need a service only for [`rho mcp serve --http`](/mcp-server#over-http), which has to listen before any client shows up. Both examples below:

- Leave the daemon running when you stop or restart the service. Stop the daemon itself with `rho daemon stop`.
- Set `PATH`. A service doesn't get your shell's environment, and the sessions it starts look up tools, MCP servers, and CLI subagents on this `PATH`. Add the directories your tools live in, and any variable your sessions need, such as a provider key. On Linux, keep `/usr/bin` in it.
- Listen on `127.0.0.1` only. To reach the server from another machine, use a tunnel ([Over HTTP](/mcp-server#over-http)).

### systemd

Save this as `~/.config/systemd/user/rho-mcp.service`. Change `ExecStart` to where `rho` is installed and `PATH` to your own:

```ini
[Unit]
Description=rho MCP server
After=network.target

[Service]
ExecStart=%h/.local/bin/rho mcp serve --http 127.0.0.1:8765
# The sessions this server starts look up executables on this PATH.
Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin
# Stop only the server. The daemon keeps running its turns.
KillMode=process
Restart=on-failure

[Install]
WantedBy=default.target
```

Start it, and keep it running across logins:

```bash
systemctl --user daemon-reload
systemctl --user enable --now rho-mcp
loginctl enable-linger "$USER"   # optional: keep it running while you are logged out
```

Every request needs a bearer token. Create one for each host that connects, naming the workspaces it may reach. It prints the secret once ([Tokens](/mcp-server#tokens)):

```bash
rho mcp token rotate laptop --scope write --workspace ~/src/app
```

`systemctl --user restart rho-mcp` restarts the server and leaves the daemon's turns running. The server logs to the journal (`journalctl --user -u rho-mcp`). The daemon logs to `~/.rho/logs/daemon.log`.

### launchd

Save this as `~/Library/LaunchAgents/dev.rho.mcp.plist`. Change the `rho` path, `PATH`, and the log path to your own:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.rho.mcp</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/homebrew/bin/rho</string>
    <string>mcp</string>
    <string>serve</string>
    <string>--http</string>
    <string>127.0.0.1:8765</string>
  </array>
  <!-- The sessions this server starts look up executables on this PATH. -->
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
  </dict>
  <!-- Stop only the server. The daemon keeps running its turns. -->
  <key>AbandonProcessGroup</key>
  <true/>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardErrorPath</key>
  <string>/Users/you/Library/Logs/rho-mcp.log</string>
</dict>
</plist>
```

Load it:

```bash
launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/dev.rho.mcp.plist
launchctl kickstart -k "gui/$(id -u)/dev.rho.mcp"   # restart the server
```

Every request needs a bearer token. Create one for each host that connects, naming the workspaces it may reach. It prints the secret once ([Tokens](/mcp-server#tokens)):

```bash
rho mcp token rotate laptop --scope write --workspace ~/src/app
```

The server logs to the `StandardErrorPath` file. The daemon logs to `~/.rho/logs/daemon.log`.
