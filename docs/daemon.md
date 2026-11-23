# Daemon

Rho runs your sessions in a background process called the daemon. The TUI, your editor's `rho acp`, and other clients connect to it. When a client quits, the daemon keeps the session and its turn running, and any client can pick it up later.

There is one daemon per user and Rho home (`~/.rho`, or `$RHO_HOME`). Rho starts it the first time a command needs it, so you rarely manage it yourself.

## Commands

| Command | What it does |
| --- | --- |
| `rho daemon status` | Show whether the daemon runs, its version, its jobs, and a pending update. Add `--json` for JSON. |
| `rho daemon start` | Start the daemon if none runs. Safe to run again. |
| `rho daemon stop` | Stop the daemon. Running turns stop and show as `interrupted`. |
| `rho daemon restart` | Restart the daemon, for example after an update. Running turns stop, then continue once the TUI reattaches ([Updates](#upgrading-in-place)). |
| `rho daemon` | Run the daemon in the foreground. Same as `rho daemon run`. |
| `rho daemon serve --http <addr>` | Expose `rho-protocol` over HTTP WebSockets (`GET /ws`) for hosts that cannot use SSH ([Over HTTP](#over-http)). |
| `rho daemon token` | Create, list, or revoke bearer tokens for `rho daemon serve --http`. |
| `rho attach <id>` | Follow a session's turn from the terminal ([Attaching](/automation-cli#attaching-to-a-daemon-session)). |
| `rho worktrees prune [--dry-run]` | Remove worktrees that are clean and merged ([Sessions in worktrees](#sessions-in-worktrees)). |
| `rho worktrees status` | Show each worktree's size and what a prune would free. |
| `rho mcp serve` | Let another agent start and follow your sessions ([MCP server](/mcp-server)). |

`rho daemon status`, `rho daemon stop`, and `rho attach` never start a daemon. With none running, they print `rho daemon is not running` and exit with status 1.

Stopping the daemon also ends the commands and background processes your sessions started. If the daemon crashes or you kill it with `SIGKILL`, those processes keep running.

## The TUI {#daemon-backed-tui}

`rho` opens the TUI as a client of the daemon. Things that work differently from Rho 2:

- Quitting the TUI leaves a running turn going. The exit message shows `rho --resume <id>`. Run it later to see the reply, even one that finished while the TUI was closed.
- A new session reaches the daemon with its first prompt. If you open `rho` and quit without typing, nothing is left behind.
- `/model`, `/permissions`, and the other settings commands change the current session. A change you make during a turn applies when the turn ends.
- A workflow or background process that a turn started keeps running after the turn ends. When it finishes, the daemon starts a new turn that gives the model the result, whether or not a TUI is open. A process started as a service, such as a dev server, starts no turn: its result waits for your next message.
- `/jobs` lists everything the daemon runs, in every project. From there you can answer questions, send messages, cancel work, and open sessions ([Interactive TUI](/interactive-tui)).
- `/login` opens the browser on the machine where the TUI runs. Sessions use the new sign-in from their next prompt.
- You can't delete a session from a picker while the daemon has it loaded.
- After `rho daemon restart`, or an update the daemon picks up on its own, the TUI reconnects by itself, and a turn the restart interrupted continues. If the daemon stops instead, after `rho daemon stop` or a mise or Windows update, reopen the session with `rho --resume <id>`.

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
| `interrupted` | The daemon stopped before the last turn ended. After `rho daemon restart`, a TUI session's turn continues once the TUI attaches again ([Updates](#upgrading-in-place)). |

A turn that waits for an approval or an answer waits until someone answers or cancels it. Rho never denies it because no client is attached. Any connected client can answer, and the first answer wins. The exception is a question with fallback answers when you set [`[questionnaire] timeout_seconds`](/configuration#questionnaire-timeout). Rho answers it with the fallback once the timeout passes.

To follow a session from a terminal, use [`rho run --detach`](/automation-cli#running-in-the-daemon) and [`rho attach`](/automation-cli#attaching-to-a-daemon-session). Editors connect through [`rho acp`](/integrations/acp). Closing an editor thread leaves its session in the daemon.

Prompting or attaching to a saved session reopens it in its own workspace, whatever directory you run from. A reopen fails when:

- another program, such as a foreground `rho run` or Rho 2, has the session open
- a subagent or workflow node owns the session, since only the session that started it can continue it
- the agent the session was saved as has changed or no longer exists

### Sessions in worktrees

A session can work in its own git worktree, so several sessions in one repository don't overwrite each other's edits. Ask for one with [`rho run --worktree`](/automation-cli#running-in-a-worktree), with `Tab` when you start a session from `/jobs`, or with the `worktree` option of the [MCP server](/mcp-server)'s `new` tool.

The daemon creates a branch `rho/<repository>-<id>` and checks it out at `~/.rho/worktrees/<repository>-<id>`. The branch starts from your `origin` remote's default branch, fetched first, so it has the latest work even when your checkout is behind. Without an `origin`, or when the fetch fails or takes over 30 seconds, it starts from your current `HEAD`. Turn off [`git.worktree_from_remote`](/configuration#git) to always start from `HEAD`. The new branch doesn't track the remote branch, so a push never lands on it. The session's commands, edits, and project instructions use the worktree. The workspace must be inside a git repository.

`rho daemon status` lists the worktrees of the daemon's sessions:

```text
  worktrees    rho/api-3fa2c1d0 /home/me/.rho/worktrees/api-3fa2c1d0
               rho/api-91bd22aa /home/me/.rho/worktrees/api-91bd22aa (stale)
```

The daemon removes a worktree once its session has sat unused for a day, the worktree has no uncommitted changes, and your checkout or the remote's default branch already has its commits. It keeps the branch, and it recreates the worktree if you reopen the session. It cleans up other worktrees under `~/.rho/worktrees` the same way, such as one from a foreground `rho run --worktree`, once no process works in it. Worktrees it can't remove are marked stale after 14 days. `rho worktrees status` shows what each one takes on disk. [Cleaning up worktrees](/automation-cli#cleaning-up-worktrees) covers the rules, `rho worktrees prune`, and how to remove a worktree yourself.

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

### Pull requests {#pull-requests}

A session's agent can watch GitHub pull requests with the [pull request tools](/tools-workspace#pull-requests), and the daemon starts a turn in the session when one changes. Only a session whose client takes daemon turns (`idle_wake: turn`), as Rho's terminal UI does, has the tools, since nothing could wake any other: a `rho run --detach`, ACP, or `idle_wake: never` session has none, and neither has one a T3 Code thread opened (`child_threads: host`), which has T3's own.

The daemon reads the watched pull requests with `gh` on its own machine every 60 seconds: a short summary of each, batched per host, then a full read of each one that changed or went 10 minutes without one.

Clients such as rho-ui also [list and show a session's pull requests](/protocol#session-pull-requests) over the daemon's socket: the session's linked pull requests, and the pull request of the branch its workspace has checked out, which the daemon looks up only for a repository whose `origin` is on github.com. The daemon reads those only while a client asks. A session a client lists is read every 5 minutes, a pull request on a client's screen every 30 seconds, and each stops two periods after the client last asked. A pull request that is linked but not watched, and that no client asks about, is never read.

Every read, watched or asked for, spends the signed-in account's GitHub API points from one budget per host. When an account has 50 points or fewer left, the daemon stops all reads of that host until GitHub resets the budget, logs one warning, and `watch_pull_request` refuses with the points left and the reset time. Clients see the pause beside each pull request on that host.

The daemon keeps each session's links, watches, and undelivered news in `<rho home>/daemon/pull-requests/` (`~/.rho/daemon/pull-requests/` by default), one file per session, so they survive a restart. News for a session the daemon unloaded reopens it for a turn, once per batch of news: if that turn fails before it takes the news, the news waits for the session's next turn. After a daemon restart, news for an unloaded session waits for its next prompt or attach.

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
- The daemon fetches a provider's model list once you sign in to it, and every signed-in provider's on `/refresh-models`. Clients share the lists it fetched, and each offers the models its own credentials reach.
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

`rho daemon restart` switches right away and interrupts running turns. Each TUI turn continues once the TUI reattaches to its session, which it does by itself: the daemon shows a notice and starts a turn that tells the model the restart cut its last turn short. That message is saved as the turn's user message, so a reopened session shows it as one. The daemon waits for that attach because a session's environment and credentials come only from its client. Only sessions whose client takes turns the daemon starts, as the TUI does, continue, and only turns without a time limit: a `rho run --detach` session stays `interrupted`. So does a session no TUI reattaches to, and every turn that a stop, a crash, or a cancel ends.

If you install with mise or Homebrew, the new version lands at a different path, and the daemon can't see it. Run `rho daemon restart` from the new version once your work is done.

A [`rho mcp serve --http`](/mcp-server#http-updates) or [`rho daemon serve --http`](#over-http) service switches to the new version by itself too: `rho mcp serve --http` once calls under way finish, and `rho daemon serve --http` once each open WebSocket has received the daemon's answers to the requests it already sent and closed with `1001` (`Away`), so clients reconnect to the replacement on the same listening socket while the daemon's turns keep running.

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
| `Service` | A documented systemd unit (`rho-mcp.service` or `rho-daemon-http.service`) is saved but not enabled ([Running as a service](#running-as-a-service)). |
| `Daemon environment` | The daemon's proxy settings or `RHO_LOG` differ from your shell's. Linux only. |
| `Daemon credential store` | The daemon can't reach its credential store ([Credentials](#credentials)). |

## Socket and permissions

Only your user can connect to the daemon. It listens on no network port.

On Linux and macOS, the daemon listens on a Unix socket, `daemon.sock`, under `$XDG_RUNTIME_DIR/rho/` or, without `XDG_RUNTIME_DIR`, under `~/.rho/run/`. Rho refuses to use the socket directory if other users can access it. Socket paths are limited to 107 bytes on Linux and 103 on macOS. If yours is longer, Rho fails with an error. Point `RHO_HOME` or `XDG_RUNTIME_DIR` at a shorter directory.

On Windows, the daemon listens on a named pipe that only your user can open. `Ctrl+C` stops a `rho daemon run` started in a console.

## Over HTTP {#over-http}

To reach the daemon from a host that cannot use SSH, run the HTTP WebSocket bridge beside it:

```bash
rho daemon token rotate laptop
rho daemon serve --http 127.0.0.1:8766
```

`rho daemon token rotate <name>` prints a `rho_daemon_` bearer secret once on stdout and keeps only its SHA-256 hash in your credential store (separate from `rho mcp token`). Manage tokens with `rho daemon token list [--json]` and `rho daemon token revoke <name>`; rotations and revocations apply on the next connection without restarting the bridge. A revoked token's open WebSockets stay connected until they close; restart the bridge to cut them off at once.

- **Access level**: a daemon HTTP token grants full user-level `rho-protocol` access (opening sessions in any workspace, answering approvals, editing config, and stopping or restarting the daemon). Treat it like SSH login for that user.
- **Binding and tunnels**: `--http` accepts only loopback (`127.0.0.1`, `::1`) or a private/overlay network address (`10.x`, `172.16–31.x`, `192.168.x`, `100.64.0.0/10` such as Tailscale or NetBird, or IPv6 ULA). On a non-loopback address traffic travels as plain HTTP/WS, so use an encrypted overlay network or put TLS in front of a loopback bind (`--allow-host <hostname>` for the public `Host` header, matching any port unless it names one, as in `--allow-host rho.example.com:443`). Browsers cannot set `Authorization` on a WebSocket, so a browser client (`--allow-origin <origin>`) needs a proxy in front of the bridge that adds the header. A `--allow-host` or `--allow-origin` value that could never match a request stops the bridge at startup.
- **Lifecycle and updates**: the bridge starts the local daemon on demand and relays each authenticated WebSocket connection (`GET /ws`) to `daemon.sock` (or the Windows named pipe). Stopping or restarting `rho daemon serve` (SIGINT or SIGTERM, or Ctrl+C on Windows) closes each open WebSocket with `1001` (`Away`) once the daemon has answered the requests it already sent, and leaves the daemon and its running turns untouched; `rho daemon stop` stops only the daemon. On macOS and Linux, once `rho update` replaces the `rho` binary in place, `rho daemon serve --http` drains active WebSockets and re-executes on the same listening socket so reconnecting clients wait in the backlog instead of being refused. Keep it running across reboots with [Running as a service](#running-as-a-service). See [Protocol over HTTP](/protocol#over-http) for the WebSocket framing contract.

## Logs

The daemon writes its log to `~/.rho/logs/daemon.log`. If you run `rho daemon run` itself as a systemd service, it logs to the journal instead. Set `RHO_LOG` to change the level, for example `RHO_LOG=rho=debug`. The default is `info`.

The log can contain provider error messages that echo credentials, so keep it private. Rho creates the file readable only by you. The journal doesn't get that protection.

## Running as a service

The daemon doesn't need a service. Rho starts it on demand. To have it running before anything connects, for example in a script, run `rho daemon start`.

You need a service only for [`rho mcp serve --http`](/mcp-server#over-http) or [`rho daemon serve --http`](#over-http), which have to listen before any client shows up. All examples below:

- Leave the daemon running when you stop or restart the service. Stop the daemon itself with `rho daemon stop`.
- Set `PATH`. A service doesn't get your shell's environment, and the sessions it starts look up tools, MCP servers, and CLI subagents on this `PATH`. Add the directories your tools live in, and any variable your sessions need, such as a provider key. On Linux, keep `/usr/bin` in it.
- Listen on `127.0.0.1`, or on this machine's private network address for a remote client or [hub](/mcp-server#several-machines) to reach it. To reach the server from anywhere else, use a tunnel ([Over HTTP](#over-http)).

### systemd

#### MCP HTTP server (`rho-mcp.service`)

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

#### Daemon HTTP WebSocket bridge (`rho-daemon-http.service`)

Save this as `~/.config/systemd/user/rho-daemon-http.service` to keep `rho-protocol` reachable over HTTP WebSockets (`GET /ws`) across reboots and auto-start the daemon on demand:

```ini
[Unit]
Description=rho daemon HTTP WebSocket bridge
After=network.target

[Service]
ExecStart=%h/.local/bin/rho daemon serve --http 127.0.0.1:8766
# Remote clients that omit environment inherit this PATH.
Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin
# Stop only the bridge. The daemon keeps running its turns.
KillMode=process
Restart=on-failure

[Install]
WantedBy=default.target
```

Enable it and issue a bearer token for each remote client:

```bash
systemctl --user daemon-reload
systemctl --user enable --now rho-daemon-http
loginctl enable-linger "$USER"   # optional: keep it running while you are logged out
rho daemon token rotate laptop
```

`systemctl --user restart rho-daemon-http` restarts the bridge and leaves the daemon's turns running. The bridge logs to the journal (`journalctl --user -u rho-daemon-http`).

### launchd

#### MCP HTTP server (`dev.rho.mcp`)

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

#### Daemon HTTP WebSocket bridge (`dev.rho.daemon-http`)

Save this as `~/Library/LaunchAgents/dev.rho.daemon-http.plist`. Change the `rho` path, `PATH`, and the log path to your own:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.rho.daemon-http</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/homebrew/bin/rho</string>
    <string>daemon</string>
    <string>serve</string>
    <string>--http</string>
    <string>127.0.0.1:8766</string>
  </array>
  <!-- Remote clients that omit environment inherit this PATH. -->
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
  </dict>
  <!-- Stop only the bridge. The daemon keeps running its turns. -->
  <key>AbandonProcessGroup</key>
  <true/>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardErrorPath</key>
  <string>/Users/you/Library/Logs/rho-daemon-http.log</string>
</dict>
</plist>
```

Load it and issue a bearer token for each remote client:

```bash
launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/dev.rho.daemon-http.plist
launchctl kickstart -k "gui/$(id -u)/dev.rho.daemon-http"   # restart the bridge
rho daemon token rotate laptop
```

The bridge logs to the `StandardErrorPath` file. The daemon logs to `~/.rho/logs/daemon.log`.
