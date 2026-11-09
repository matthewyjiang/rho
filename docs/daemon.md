# Daemon

The rho daemon is one long-lived background process per user and rho home (`~/.rho`, or `$RHO_HOME`). It owns agent work so that frontends (the interactive TUI, `rho acp`, and other clients) can come and go without stopping it. Frontends talk to it over a private Unix socket using rho's versioned JSON-RPC protocol, described in the [protocol reference](/protocol) and published as the [`rho-protocol`](https://crates.io/crates/rho-protocol) crate.

## Commands

| Command | What it does |
| --- | --- |
| `rho daemon` | Run the daemon in the foreground until it is stopped. Same as `rho daemon run`. `rho daemon run --unload-idle-after <DURATION>` sets how long a session sits idle before it is [unloaded](#unloading-idle-sessions), or discarded if it saved nothing (`60s` by default). |
| `rho daemon start` | Start the daemon in the background unless one is running, wait until it answers, and print its pid. Run again, it starts no second daemon and still exits 0, so scripts can run it unconditionally. |
| `rho daemon status` | Show the running daemon's pid, version, protocol, uptime, jobs, and socket. Add `--json` for one JSON document. |
| `rho daemon stop` | Ask the daemon to shut down and wait for it to exit. |
| `rho mcp serve` | Serve the daemon's sessions as an MCP server, so another agent can start, follow, and steer them. See [MCP server](/mcp-server). |
| `rho attach <ID>` | Follow a session's turn until it ends, and exit as it ended; answer its requests, steer it, or cancel it from the terminal; Ctrl+C detaches. See [Attaching to a daemon session](/automation-cli#attaching-to-a-daemon-session). |

`status`, `stop`, and `attach` never start a daemon. With none running, they print `rho daemon is not running` and exit with status 1.

Only one daemon runs per rho home. A second `rho daemon run` fails with the pid, version, and socket of the one already running.

The daemon shuts down gracefully on `rho daemon stop`, `SIGTERM`, or `SIGINT`: it detaches every client following a session with `rho/session/detached` (reason `daemon_stopping`), stops accepting connections and reading requests, answers the requests it has already received, and removes its socket. A client that has stopped reading, such as a suspended TUI, is dropped once a message to it has waited 1 second, so it cannot hold the stop open. It then stops the [turns](#sessions-and-jobs) still running, waits up to 4 seconds for them to save, records them `interrupted`, and exits with status 0. A request that never finishes holds that shutdown open. A `SIGTERM` or `SIGINT` that arrives once shutdown is under way, whether a second signal or any signal after `rho daemon stop`, makes the daemon exit at once with status 1; the next daemon to start removes the socket it left behind and records the turns that daemon left open as `interrupted`.

Stopping the daemon ends every process its sessions' tools started: the command a turn is running and the background processes a session's `process` tool started, each with its whole process group, so their children go too. A process that left its group, for example through `setsid`, keeps running. Unloading an idle session ends none of them, since a session whose background process still runs is never idle. A daemon that crashes, is killed with `SIGKILL`, or is forced to exit by a second signal cannot end them: its tools' processes keep running, as they do when any rho process is killed that way, and the next daemon does not track them.

## Experimental daemon-backed TUI

On Unix, `RHO_TUI_DAEMON=1 rho` opens the interactive TUI as a daemon client. Without that environment variable, the TUI keeps its in-process behavior. The flag is an unfinished migration, not a drop-in replacement yet.

The attached session supplies the current model, reasoning level, fast mode, permission mode, and their available choices. `/model`, `/fast`, `/permissions`, the limited `/config` menu, and the model, reasoning, and permission shortcuts configure that session rather than the unused local runtime. Changes requested during a turn apply after it ends; the TUI remains responsive to approvals and cancellation, and displays the old settings until the daemon confirms the change. Changes are session settings, not edits to the default configuration. Context tokens and cumulative session cost feed the usual context and spend gauges, and title changes update the followed session's title live.

The command menu includes commands the daemon advertises, such as `/init`, `/compact`, prompt templates, MCP prompts, and command workflows. These are resolved by the daemon. Pure UI commands remain local. `/goal` runs its loop in the daemon, so a goal keeps working after the TUI quits; see [Goals](/protocol#goals). Unported runtime commands—including advisor controls, session navigation and rewind, login/logout, the agents and workflow hubs, `/remember`, `/context`, `/add-dir`, and `!`/`!!` shells—are refused with `/<cmd> is not available with RHO_TUI_DAEMON yet`. Full provider and runtime configuration, startup model/reasoning/permission overrides, and reconnection after a lost connection remain unported.

`/mcp` opens the existing inventory overlay using the attached daemon session's servers, not the unused local runtime or a freshly read client configuration. It includes disabled and failed servers, exported tools, live tool changes and keepalive details, and current prompt/resource inventory. Each opening requests a fresh snapshot with `rho/session/mcp`; it remains usable during a turn even when a settings change is waiting to apply. Close and reopen the overlay to refresh it.

Quitting detaches and leaves any running turn in the daemon. The exit receipt shows `rho --resume <short id>` once a new session has a saved prompt; quitting an untouched session shows no resume hint. Reopening with `RHO_TUI_DAEMON=1 rho --resume <id>` catches up with the turn, including a reply completed while the TUI was closed.

If the daemon detaches a client because it fell behind (`lagged`), the TUI automatically attaches again after its last event. A tail replay extends the existing transcript; a reset replay replaces it when the old cursor is no longer available. Open input requests and turn state are restored from the attachment. A failed reattach stops following and shows an error rather than retrying indefinitely; other detach reasons and a lost connection still stop following.

## Sessions and jobs

A session is a conversation: its saved transcript and, while the daemon has it loaded, its live runtime. Requests name a session by its session id, the id `rho sessions` and `/resume` show. A loaded session's turns run in the daemon, so a turn keeps running when the client that started it disconnects, and another client can follow it later. The daemon tracks every unit of work it runs as a job with its own id: the turns of one session, or one subagent run that a session started.

Clients open, prompt, cancel, steer, withdraw steers from, and follow sessions with the `rho/session/*` methods, list, rename, and delete saved sessions, loaded or not, with `rho/session/list`, `rho/session/rename`, and `rho/session/delete`, and list jobs with `rho/job/list`; the [protocol reference](/protocol) describes every method, its errors, and the events a client following a session receives.

A session opens with the configuration, agents, and instructions that `rho` launched in its workspace would use, read when the session is opened. It runs as the agent its client names, else the default agent, with the directories, MCP servers, and parts left out its client chose ([protocol](/protocol#a-session-s-setup)) and the configured permission mode, until a client changes its model, reasoning level, fast mode, or permission mode between turns with `rho/session/configure` ([protocol](/protocol#changing-a-session-s-settings)). A prompt's `content` reaches the model as the [protocol reference](/protocol#content-blocks) describes. A prompt that starts with a slash command resolves as in [`rho run`](/automation-cli#sessions-commands-and-questions): `/init`, prompt templates, MCP prompts, `$skill` mentions, `/compact`, `/workflow:<name>`, and `/agent:<name>` work, while the TUI's own commands are refused ([protocol](/protocol#methods)). The session's file is written with its first prompt, so `rho sessions` lists it from then on. After its first answered turn, the daemon names the session as the TUI would, and attached clients get the title as a `title_changed` event; clients that attach later find it in the attach response.

A session's job state is the state of its current or last turn:

| State | Meaning |
| --- | --- |
| `idle` | Opened; no turn has started. |
| `running` | A turn is under way. |
| `waiting_for_input` | The turn waits for a tool approval, a plan approval, or an answer to a question. |
| `completed` | The last turn finished. |
| `failed` | The last turn failed; the job's `error` says why. |
| `cancelled` | Someone cancelled the last turn. |
| `timed_out` | The daemon cancelled the last turn at the time limit its prompt set, as `rho run --detach --timeout` does. |
| `interrupted` | The daemon stopped, or died, before the last turn ended. |

A new prompt moves a session's job from any state but `running` and `waiting_for_input` back to `running`. `rho daemon status` counts the jobs that are `queued`, `running`, or `waiting_for_input`.

### Subagents

Subagents that a session starts run in the daemon, and each run is a job of kind `subagent` whose `parent` is the session's job. Its `session` is the session the subagent writes; a subagent on another runtime (Claude, Cursor, Antigravity) keeps that runtime's own and has none. Its state is that of its one run: `queued` until it gets a slot, then `running`, then `completed`, `failed`, `cancelled`, or `interrupted`. A session whose client's host runs its children, as [T3 Code](/integrations/acp/t3-code#subagents-run-as-t3-threads) does, starts no subagent jobs for its `agent` tool: those children are the host's threads. The agent nodes of its workflow runs still run as subagent jobs.

Every session's subagents, and the agent nodes of the workflow runs it starts, share one budget, `agent_concurrency` in your config (10 by default), so at most that many run at once across all your projects. Claude-cli subagents first take a slot in a nested budget, 2 by default and never more than `agent_concurrency` (`RHO_CLAUDE_AGENT_CONCURRENCY` in the daemon's environment), then wait for a slot in the shared one. Each budget gives its slots in the order runs joined its queue. While a run waits, its job usually lists its `queue` place (it can be briefly absent, for example while a run moves between budgets): the budget it waits on, that budget's limit, and its position in line. The daemon reads `agent_concurrency` each time it opens a session, so an edit applies to every session's subagents from the next session you open; runs already going keep their slots.

### Workflow runs

A workflow run a session starts with the `workflow` tool (`start`, `run`, or `resume`) runs in the daemon, in the session's workspace, and is a job of kind `workflow` whose `parent` is the session's job and whose `workflow_run` names the run. It is `running` while the daemon drives the run, then `completed` when the run succeeded, `cancelled` when it was cancelled, or `failed`, with the reason, when it failed, was denied or blocked, or could not be driven. Each agent node is a `subagent` job whose `parent` is the workflow job, and takes its slot in the shared budget like any subagent. A node's `timeout_seconds` counts from when it joins the line, so a wait for a slot uses part of it; a node that times out before it gets one fails naming the budget, its limit, and the node's place in line.

A run's capability approvals follow its permission mode as for any background run: nobody is asked. A run the daemon's stop cuts short ends `interrupted` and resumes like a run whose process died: the session's `workflow` tool or `rho workflow resume` picks it up, with `recover_uncertain` (`--recover-uncertain`) once you have checked that no process an interrupted node started is still running. The daemon does not resume runs on its own. A resume through a daemon session's `workflow` tool drives the run in the daemon as a new `workflow` job; `rho workflow resume` drives it in your terminal.

A session's turn that asks for approval, a plan approval, or answers to questions waits in `waiting_for_input` until a client answers or the turn is cancelled. It is never denied because no client is attached: a client that leaves and comes back finds it still open. The one exception is the user's own: with [`[questionnaire] timeout_seconds`](/configuration#questionnaire-timeout) set, a question whose agent offered fallback answers is answered with them once the timeout passes, unless a client answers or [holds](/protocol#timeouts) it first. Any connected client can answer, whether or not it follows the session, and the first answer wins; see [Answering a turn's requests](/protocol#answering-a-turn-s-requests).

### Following a session

Any number of clients can follow a session with `rho/session/attach`: they receive its conversation as events, live, and a client that comes back resumes where it left off, or gets the saved conversation again when the daemon no longer buffers what it missed. See [Following a session](/protocol#following-a-session) in the protocol reference. From a terminal, [`rho run --detach`](/automation-cli#running-in-the-daemon) starts one and [`rho attach`](/automation-cli#attaching-to-a-daemon-session) follows it. Editors and desktop apps open, load, and follow sessions through [`rho acp`](/integrations/acp), which starts the daemon when none runs; closing a thread there detaches, and the session stays in the daemon.

Each job's record is a JSON file in `~/.rho/daemon/jobs/` (`$RHO_HOME/daemon/jobs/` with `RHO_HOME`), rewritten on every state change. Records are kept, even when their session is deleted, so a daemon lists the jobs earlier daemons ran; only the job of a session the daemon [discards unsaved](#unloading-idle-sessions) goes. A prompt or an attach for a saved session the daemon has not loaded, such as one an earlier daemon ran or one the TUI saved, reopens it in its own workspace, whatever workspace the client is in. It runs as the agent it was saved as, and one `/agent:<name>` rebound runs as that agent again under the tools of the agent it was rebound from, never more; if either agent's definition changed or is gone, the reopen fails with `SessionOpenFailed` (-32008), as the TUI's resume does, rather than run the session as another agent. It keeps the directories and parts left out it was saved with, adding those of the attach that reopens it, and takes MCP servers and whether its client shows questions from that attach alone, so a prompt reopens it without them ([protocol](/protocol#a-session-s-setup)). Its next turns are recorded on its last session job, so a session keeps one job across daemons; a session no daemon ran gets a new one. Cancel, steer, unsteer, answer, hold, and `rho/session/configure` never reopen a session, since one that is not loaded has no turn to act on; a client attaches before it changes a session's settings. Nor does `rho/session/observe`, which a client that only reads a session uses: it replays an unloaded session from its file and leaves it unloaded, starting none of its MCP servers, OpenAPI sources, or hooks.

A reopen is refused when:

- another process, such as the TUI or a foreground `rho run`, has the session open: `SessionInUse` (-32012);
- another job of the daemon's, such as a subagent run, writes it: `SessionBusy` (-32004);
- it is a subagent's or workflow node's session, which only the session that started it continues: `SessionOpenFailed` (-32008);
- nothing is saved under the id: `SessionNotFound` (-32003).

While the daemon has a session loaded, other processes cannot open it; see [Unloading idle sessions](#unloading-idle-sessions). A reopened session's job state is the daemon's last record of it: a session continued in the TUI since then still reports how the daemon's last turn ended, until the TUI runs through the daemon ([#1667](https://github.com/matthewyjiang/rho/issues/1667)).

### Unloading idle sessions

A session sits idle while no turn is under way, no subagent or workflow node writes it, no client follows it, and no subagent, workflow run, or background process its prompts started is still running or holding a result for the next prompt. Once a session has been idle for 60 seconds, the daemon unloads it: it closes the session's MCP servers and lets go of the session's file, so the TUI or another rho process can open it. The session's job and saved conversation stay, and the next prompt or attach reopens it, which takes a few tens of milliseconds plus the startup of its MCP servers. The daemon checks once per idle period, so a session unloads between one and two idle periods after it went idle, and the daemon log records each unload with how long the session was idle. A session that has not saved its file yet, because it has had no prompt or its turns saved nothing, has nothing to reopen from, so the daemon discards it instead and logs it as discarded: it closes the session's MCP servers and forgets its job, so `rho/job/list` stops listing it and a later prompt or attach gets `SessionNotFound`. A client that follows an unsaved session keeps it loaded, as it keeps any session. `rho daemon run --unload-idle-after <DURATION>`, such as `--unload-idle-after 10m`, changes the idle period (at least `100ms`); a daemon that a frontend started automatically uses the default.

A reopen starts the session afresh from its file and the current config. Settings a client chose with `rho/session/configure` carry over: the session keeps its model, reasoning level, fast mode, and permission mode with its file from the first change on, as they stand at each save, a plan approval's mode switch included. A session no client changed takes the configured settings again. Approvals granted for the session do not carry over, and a session whose agent definition changed since is refused, as for any reopen.

### Environment

A session's commands run with its client's environment, not the daemon's: each frontend sends its environment variables when it opens, prompts, or attaches to a session, so `bash` finds the same `PATH`, mise shims, `SSH_AUTH_SOCK`, and virtualenv as the terminal you prompted from ([#1659](https://github.com/matthewyjiang/rho/issues/1659)). Each prompt brings the prompting client's environment, so after `mise use` or `export FOO=bar` the next prompt's commands see the change, including those of subagents still running from an earlier prompt; a background process keeps the environment it started with. A command you allowed for the session is asked about again when a prompt brings a different environment, since the environment can change what the command does. An attach changes the environment only when it reopens a session the daemon had unloaded. Provider credentials, such as an exported `OPENAI_API_KEY`, are left out, as tools in every rho session go without them, and the daemon's own credential lookups still read the daemon's environment. The daemon never logs or saves the environment, so a session reopened after a restart takes the environment of the client that reopens it.

A session's MCP servers, hooks, and OpenAPI sources use its environment too. MCP servers and OpenAPI sources take the environment current when the session's tool sources load: when the session opens or reopens, the opening client's, or when `/agent:<name>` rebinds it mid-prompt, the prompting client's. An MCP server's executable is looked up on that environment's `PATH`, and the server takes its base variables, `env_from_env`, and `headers_from_env` from it and keeps them while it runs; an OpenAPI source reads its environment variables from it. A hook takes its base variables and its `env` names from the environment current when it runs. A variable you name this way is read even when it is a provider credential. A session opened or reopened by a client whose environment sets `RHO_IN_HOOK`, such as `rho` run by a hook, runs no hooks, so a hook cannot trigger itself through the daemon; a daemon a hook started automatically does not inherit the marker, so the daemon's other sessions keep theirs.

CLI subagents ([Claude Code](/subagents/claude-cli), [Cursor](/subagents/cursor), and [Antigravity](/subagents/antigravity)) and a [Claude Code advisor](/configuration/advisor-mode#claude-code-as-the-advisor) use the session's environment as it is when they start: their executable is looked up on its `PATH`, and they run with all of it, provider credentials such as `ANTHROPIC_API_KEY` or `CURSOR_API_KEY` included, since they sign in themselves, as they would from your terminal. A workflow run that a session's `workflow` tool or `/workflow:<name>` starts or resumes takes the session's environment as it is at that moment and keeps it until the run stops: its hooks, command nodes, rho agent nodes, and CLI agent nodes all run in it, and later prompts do not change it. Planning looks up command executables, their `#!/usr/bin/env` interpreters, and CLI agent binaries on that environment's `PATH` and freezes them, and each node checks them again on the same `PATH` before it starts. A run you start or resume with `rho workflow` uses your terminal's environment instead. Rho never saves the environment with a run, so a run resumed from another session or terminal takes that one's; when its `PATH` finds a different interpreter for a frozen script, the run fails closed with a `frozen filesystem identity drift` error rather than running a script under an interpreter it was not planned with. Project trust (`RHO_TRUST_PROJECT_AGENTS` and the other `RHO_TRUST_PROJECT_*` variables) is still read from the daemon's environment, because a session loads its agents, plugins, and hooks once when it opens while its environment can change with every prompt; set them in the environment the daemon starts in.

## Starting automatically

Frontends that use the daemon, such as `rho run --detach` and `rho daemon start`, start it when none is running, so you rarely run `rho daemon` yourself. If several start at once, they agree on a single daemon. A frontend that finds the previous daemon still shutting down waits for it to exit, up to 5 seconds, and then starts the new one; a daemon still holding its lock after that fails the frontend with the limit and the time it waited. The started daemon runs `rho daemon run` from the same `rho` executable in its own process group, so `Ctrl+C` in your terminal does not stop it.

## Versions

Every connection begins with a handshake that compares protocol versions. A frontend and daemon work together when their protocol major versions match. Otherwise the frontend reports which side to change:

- The daemon is older, for example after you update rho while it keeps running: run `rho daemon stop`, and the next connection starts the new version.
- The frontend is older: update rho.

What each protocol version may change is described in [Versions and compatibility](/protocol#versions-and-compatibility).

## Socket and permissions

The daemon's socket, `daemon.sock`, lives in a runtime directory of its own rho home:

- `$XDG_RUNTIME_DIR/rho/<home id>` when `XDG_RUNTIME_DIR` is set to an absolute path, as on most Linux desktops. `<home id>` is the first 16 hex digits of the SHA-256 of the rho home's real path, so every rho home gets its own socket.
- Otherwise `<rho home>/run`.

`RHO_HOME` only picks the rho home; the socket follows from the home as above.

The rest of the daemon's files live in the rho home's state directory, `~/.rho/daemon` (`$RHO_HOME/daemon` with `RHO_HOME`):

| File | Purpose |
| --- | --- |
| `daemon.lock` | Held by the running daemon, so only one daemon runs per rho home even when two resolve different runtime directories. |
| `daemon.json` | The running daemon's pid, version, protocol, start time, and socket. |
| `jobs/` | One record per job (see [Sessions and jobs](#sessions-and-jobs)). |

A `spawn.lock` in the runtime directory lets concurrent frontends start exactly one daemon.

Only your user can reach the daemon:

- The runtime directory and the state directory are created with mode `0700`. If one already exists, rho refuses to use it unless it is a real directory (not a symlink) that you own and that grants no group or other access.
- The socket has mode `0600`.
- The daemon checks the user id of every connecting process and closes connections from other users.
- The daemon listens on no TCP port.

Unix socket paths are limited to 107 bytes on Linux and 103 on macOS. If the socket path would be longer, rho fails with the limit and the actual length; point `RHO_HOME` or `XDG_RUNTIME_DIR` at a shorter directory.

The daemon is not supported on Windows yet ([#1665](https://github.com/matthewyjiang/rho/issues/1665)), and neither are the clients that need it, such as `rho acp`.

## Logs

When the daemon runs under systemd with stderr connected to the journal (`JOURNAL_STREAM` matches stderr), it logs to stderr and journald records it. Otherwise it appends to `~/.rho/logs/daemon.log` (`$RHO_HOME/logs/daemon.log` with `RHO_HOME`).

`RHO_LOG` sets the log filter with the same syntax as for other rho commands, for example `rho=debug`. The daemon defaults to `info`.

## Credentials

The daemon's sessions read provider credentials from the [credential store](/authentication-and-models#where-credentials-live) the daemon opened, and a variable in the daemon's own environment, such as `OPENAI_API_KEY`, wins over a saved credential. A frontend signs the daemon in with an API key, or out, through [`rho/auth/key` and `rho/auth/logout`](/protocol#signing-in-and-out), or runs a provider's interactive login in the daemon through [`rho/auth/login`](/protocol#logging-in), which relays the login's URL and code to the frontend; `rho login` does that, starting the daemon when none runs ([#1662](https://github.com/matthewyjiang/rho/issues/1662)), and so does [`rho acp`](/integrations/acp#sign-in) for an ACP host's sign-in. Each one writes the store the config selects when it runs, a login once the provider approved it, and applies to every session the daemon runs from that session's next prompt, whichever client opened it; a refresh of a provider's OAuth tokens does too. Any such change rebuilds every running session's provider at its next prompt, never during a turn, whichever auth profile it uses. MCP servers' OAuth tokens and web-search keys feed no provider, so saving them rebuilds nothing. A key set in the daemon's environment cannot be replaced or removed this way: unset it where the daemon starts and restart the daemon.

A `/login` or `/logout` in a TUI runs in that process until the TUI becomes a daemon client ([#1667](https://github.com/matthewyjiang/rho/issues/1667)). It changes the same store, but providers keep the credentials they were built with, so the daemon's running sessions do not notice at once. New sessions read the change. A running session rereads the store once the provider refuses its credentials, so a new login fixes it at the next prompt, while a session signed out elsewhere keeps working until the provider refuses its key.

## Configuration changes

The daemon reads [configuration](/configuration) when it opens a session, so edits apply to new sessions without a restart. Daemon-level settings, which are the `RHO_LOG` filter, the socket location (`XDG_RUNTIME_DIR`, `RHO_HOME`), and the credential store, are read once at startup. Restart the daemon with `rho daemon stop` to change them; a [sign-in or sign-out](#credentials) through the daemon also moves it to the credential store the config selects then. New sessions read that store, while running sessions keep the credentials they hold until a write moves them or their provider refuses them.

## Running as a service

The daemon needs no service of its own: every frontend starts it on demand. Only a process that has to listen before any client shows up needs a service manager, and today that is [`rho mcp serve --http`](/mcp-server#over-http). It is a daemon client like any other, so its first tool call starts the daemon from inside its service, and its unit is all the always-on setup there is. Use `rho daemon start` in a script that wants the daemon running before anything connects.

Both examples below make the same three choices:

- **Stopping the service leaves the daemon running.** The daemon the server starts runs in a process group of its own, but systemd stops every process in a service's control group by default, and a new process group does not leave it: restarting or stopping the MCP server would stop the daemon and every turn it runs. `KillMode=process` makes systemd stop only the server. launchd kills what is left of a job's process group when the job exits; the daemon is not in the server's group, and `AbandonProcessGroup` keeps launchd from killing anything the server leaves behind all the same. Stop the daemon itself with `rho daemon stop`.
- **`PATH` is set.** A service starts with the service manager's bare environment, not your shell's, and the sessions the server starts run with the server's environment: their tools, [MCP servers](/integrations/mcp), and [CLI subagents](/subagents) find executables on its `PATH` (see [Environment](#environment)). Set it to the directories your tools live in, and add any variable sessions need that your shell would give them, such as a provider key.
- **The server listens on loopback.** Reach it from elsewhere through a tunnel, as [Over HTTP](/mcp-server#over-http) describes.

### systemd

Save as `~/.config/systemd/user/rho-mcp.service`, changing `ExecStart` to where `rho` is installed and `PATH` to your own:

```ini
[Unit]
Description=rho MCP server
After=network.target

[Service]
ExecStart=%h/.local/bin/rho mcp serve --http 127.0.0.1:8765
# The sessions this server starts look up executables on this PATH.
Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin
# Stop only the server: the daemon it started keeps running its turns.
KillMode=process
Restart=on-failure

[Install]
WantedBy=default.target
```

Then start it, and keep it running across logins:

```bash
systemctl --user daemon-reload
systemctl --user enable --now rho-mcp
loginctl enable-linger "$USER"   # optional: keep it running while you are logged out
```

`systemctl --user restart rho-mcp` restarts the server and leaves the daemon and its running turns alone. The server logs to the journal (`journalctl --user -u rho-mcp`); the daemon it starts writes its own log to `~/.rho/logs/daemon.log`, as [Logs](#logs) describes.

### launchd

Save as `~/Library/LaunchAgents/dev.rho.mcp.plist`, changing the `rho` path, `PATH`, and the log path to your own:

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
  <!-- Stop only the server: the daemon it started keeps running its turns. -->
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

Then load it:

```bash
launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/dev.rho.mcp.plist
launchctl kickstart -k "gui/$(id -u)/dev.rho.mcp"   # restart the server
```

The server logs to `StandardErrorPath`; the daemon it starts writes its own log to `~/.rho/logs/daemon.log`, as [Logs](#logs) describes.
