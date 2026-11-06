# Daemon

The rho daemon is one long-lived background process per user and rho home (`~/.rho`, or `$RHO_HOME`). It owns agent work so that frontends (the interactive TUI, `rho acp`, and other clients) can come and go without stopping it. Frontends talk to it over a private Unix socket using rho's versioned JSON-RPC protocol, described in the [protocol reference](/protocol) and published as the [`rho-protocol`](https://crates.io/crates/rho-protocol) crate.

## Commands

| Command | What it does |
| --- | --- |
| `rho daemon` | Run the daemon in the foreground until it is stopped. Same as `rho daemon run`. |
| `rho daemon status` | Show the running daemon's pid, version, protocol, uptime, jobs, and socket. Add `--json` for one JSON document. |
| `rho daemon stop` | Ask the daemon to shut down and wait for it to exit. |

`status` and `stop` never start a daemon. With none running, they print `rho daemon is not running` and exit with status 1.

Only one daemon runs per rho home. A second `rho daemon run` fails with the pid, version, and socket of the one already running.

The daemon shuts down gracefully on `rho daemon stop`, `SIGTERM`, or `SIGINT`: it detaches every client following a session with `rho/session/detached` (reason `daemon_stopping`), stops accepting connections and reading requests, answers the requests it has already received, and removes its socket. A client that has stopped reading, such as a suspended TUI, is dropped once a message to it has waited 1 second, so it cannot hold the stop open. It then stops the [turns](#sessions-and-jobs) still running, waits up to 4 seconds for them to save, records them `interrupted`, and exits with status 0. A request that never finishes holds that shutdown open. A `SIGTERM` or `SIGINT` that arrives once shutdown is under way, whether a second signal or any signal after `rho daemon stop`, makes the daemon exit at once with status 1; the next daemon to start removes the socket it left behind and records the turns that daemon left open as `interrupted`.

## Sessions and jobs

A session is a conversation: its saved transcript and, while the daemon has it loaded, its live runtime. Requests name a session by its session id, the id `rho sessions` and `/resume` show. A loaded session's turns run in the daemon, so a turn keeps running when the client that started it disconnects, and another client can follow it later. The daemon tracks every unit of work it runs as a job with its own id: the turns of one session, or one subagent run that a session started.

Clients open, prompt, cancel, steer, and follow sessions with the `rho/session/*` methods, and list jobs with `rho/job/list`; the [protocol reference](/protocol) describes every method, its errors, and the events a client following a session receives.

A session opens with the configuration, agents, and instructions that `rho` launched in its workspace would use, read when the session is opened. It runs as the default agent with the configured permission mode. `text` reaches the model as written: slash commands are not expanded yet. The session's file is written with its first prompt, so `rho sessions` lists it from then on.

A session's job state is the state of its current or last turn:

| State | Meaning |
| --- | --- |
| `idle` | Opened; no turn has started. |
| `running` | A turn is under way. |
| `waiting_for_input` | The turn waits for a tool approval, a plan approval, or an answer to a question. |
| `completed` | The last turn finished. |
| `failed` | The last turn failed; the job's `error` says why. |
| `cancelled` | Someone cancelled the last turn. |
| `interrupted` | The daemon stopped, or died, before the last turn ended. |

A new prompt moves a session's job from any state but `running` and `waiting_for_input` back to `running`. `rho daemon status` counts the jobs that are `queued`, `running`, or `waiting_for_input`.

### Subagents

Subagents that a session starts run in the daemon, and each run is a job of kind `subagent` whose `parent` is the session's job. Its `session` is the session the subagent writes; a subagent on another runtime (Claude, Cursor, Antigravity) keeps that runtime's own and has none. Its state is that of its one run: `queued` until it gets a slot, then `running`, then `completed`, `failed`, `cancelled`, or `interrupted`.

Every session's subagents share one budget, `agent_concurrency` in your config (10 by default), so at most that many subagents run at once across all your projects. Claude-cli subagents first take a slot in a nested budget, 2 by default and never more than `agent_concurrency` (`RHO_CLAUDE_AGENT_CONCURRENCY` in the daemon's environment), then wait for a slot in the shared one. Each budget gives its slots in the order runs joined its queue. While a run waits, its job lists its `queue` place: the budget it waits on, that budget's limit, and its position in line. The daemon reads `agent_concurrency` each time it opens a session, so an edit applies to every session's subagents from the next session you open; runs already going keep their slots.

Clients cannot yet answer a turn's requests ([#1660](https://github.com/matthewyjiang/rho/issues/1660)). A turn that asks for approval or input waits in `waiting_for_input` until a client can answer; it is never denied because no client is attached. Until then, only `rho/session/cancel` ends the wait.

### Following a session

Any number of clients can follow a session with `rho/session/attach`: they receive its conversation as events, live, and a client that comes back resumes where it left off, or gets the saved conversation again when the daemon no longer buffers what it missed. See [Following a session](/protocol#following-a-session) in the protocol reference.

Each job's record is a JSON file in `~/.rho/daemon/jobs/` (`$RHO_HOME/daemon/jobs/` with `RHO_HOME`), rewritten on every state change. Records are kept: nothing deletes them yet, so a daemon lists the jobs earlier daemons ran. A daemon serves only the sessions it opened itself; it cannot load a saved session yet ([#1661](https://github.com/matthewyjiang/rho/issues/1661)). A request for a session it has not loaded, such as one an earlier daemon ran, one the TUI saved, or one that does not exist, is refused with `SessionNotLoaded` (-32003); telling those apart comes with reopen.

Tools in a session run with the daemon's own environment, minus provider credentials, as tools in every rho session do. A daemon that a frontend started automatically has that frontend's environment. Clients will send their own environment later ([#1659](https://github.com/matthewyjiang/rho/issues/1659)).

## Starting automatically

Frontends that use the daemon start it when none is running, so you rarely run `rho daemon` yourself. If several start at once, they agree on a single daemon. A frontend that starts one while the previous daemon is still shutting down can time out waiting for it; [#1657](https://github.com/matthewyjiang/rho/issues/1657) tracks waiting for the old daemon instead. The started daemon runs `rho daemon run` from the same `rho` executable in its own process group, so `Ctrl+C` in your terminal does not stop it.

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

The daemon is not supported on Windows yet ([#1665](https://github.com/matthewyjiang/rho/issues/1665)).

## Logs

When the daemon runs under systemd with stderr connected to the journal (`JOURNAL_STREAM` matches stderr), it logs to stderr and journald records it. Otherwise it appends to `~/.rho/logs/daemon.log` (`$RHO_HOME/logs/daemon.log` with `RHO_HOME`).

`RHO_LOG` sets the log filter with the same syntax as for other rho commands, for example `rho=debug`. The daemon defaults to `info`.

## Configuration changes

The daemon reads [configuration](/configuration) when it opens a session, so edits apply to new sessions without a restart. Daemon-level settings, which are the `RHO_LOG` filter, the socket location (`XDG_RUNTIME_DIR`, `RHO_HOME`), and the credential store, are read once at startup. Restart the daemon with `rho daemon stop` to change them.

## Running as a service

systemd user units and launchd agents for the daemon are planned ([#1663](https://github.com/matthewyjiang/rho/issues/1663), [#1664](https://github.com/matthewyjiang/rho/issues/1664)).
