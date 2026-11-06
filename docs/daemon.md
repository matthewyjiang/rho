# Daemon

The rho daemon is one long-lived background process per user and rho home (`~/.rho`, or `$RHO_HOME`). It owns agent work so that frontends (the interactive TUI, `rho acp`, and other clients) can come and go without stopping it. Frontends talk to it over a private Unix socket using rho's versioned JSON-RPC protocol, published as the [`rho-protocol`](https://crates.io/crates/rho-protocol) crate.

## Commands

| Command | What it does |
| --- | --- |
| `rho daemon` | Run the daemon in the foreground until it is stopped. Same as `rho daemon run`. |
| `rho daemon status` | Show the running daemon's pid, version, protocol, uptime, jobs, and socket. Add `--json` for one JSON document. |
| `rho daemon stop` | Ask the daemon to shut down and wait for it to exit. |

`status` and `stop` never start a daemon. With none running, they print `rho daemon is not running` and exit with status 1.

Only one daemon runs per rho home. A second `rho daemon run` fails with the pid and version of the one already running.

The daemon shuts down gracefully on `rho daemon stop`, `SIGTERM`, or `SIGINT`: it stops accepting connections and reading requests, answers the requests it has already received, and removes its socket. It then cancels the [turns](#threads-and-jobs) still running, waits up to 4 seconds for them to save, records them `interrupted`, and exits with status 0. A request that never finishes holds that shutdown open. A `SIGTERM` or `SIGINT` that arrives once shutdown is under way, whether a second signal or any signal after `rho daemon stop`, makes the daemon exit at once with status 1; the next daemon to start removes the socket it left behind and records the turns that daemon left open as `interrupted`.

## Threads and jobs

A thread is a conversation the daemon hosts: one saved session, prompted turn by turn. Its turns run in the daemon, so a turn keeps running when the client that started it disconnects, and another client can follow it later. The daemon tracks every unit of work it runs as a job; today every job is a thread.

The [`rho-protocol` `methods` module](https://docs.rs/rho-protocol/latest/rho_protocol/methods/) is the full method reference. The thread and job methods:

| Method | Params | What it does |
| --- | --- | --- |
| `rho/thread/create` | `cwd` | Opens a thread on a new session in `cwd`, an absolute path to an existing directory. Returns the thread's job id and session id. |
| `rho/thread/prompt` | `thread`, `text` | Starts a turn and returns once the turn has saved the prompt. A thread runs one turn at a time; a prompt while a turn is under way is refused. |
| `rho/thread/cancel` | `thread` | Cancels the turn under way, if any. The turn ends `cancelled`, or `interrupted` when the daemon is stopping. A cancel sent before a prompted turn begins still cancels it. |
| `rho/thread/steer` | `thread`, `text` | Adds `text` to the running turn's next model request. Refused when no model request is running. |
| `rho/jobs/list` | | Lists every job the daemon knows, including those earlier daemons ran. |

A prompt whose turn ends before it saves the prompt returns an error instead of a result: `TurnFailed` (-32009) with the reason, `TurnCancelled` (-32010) when a cancel stopped it, or `DaemonStopping` (-32007) when the daemon is stopping.

A thread opens with the configuration, agents, and instructions that `rho` launched in `cwd` would use, read when the thread is created. It runs as the default agent with the configured permission mode. `text` reaches the model as written: slash commands are not expanded yet.

A thread's job state is the state of its current or last turn:

| State | Meaning |
| --- | --- |
| `idle` | Created; no turn has started. |
| `running` | A turn is under way. |
| `waiting_for_input` | The turn waits for a tool approval, a plan approval, or an answer to a question. |
| `completed` | The last turn finished. |
| `failed` | The last turn failed; the job's `error` says why. |
| `cancelled` | The last turn was cancelled. |
| `interrupted` | The daemon stopped, or died, before the last turn ended. |

A new prompt moves a thread from any state but `running` and `waiting_for_input` back to `running`. `rho daemon status` counts the jobs that are `running` or `waiting_for_input`.

Clients cannot yet watch a turn's events ([#1657](https://github.com/matthewyjiang/rho/issues/1657)) or answer its requests ([#1660](https://github.com/matthewyjiang/rho/issues/1660)). A turn that asks for approval or input waits in `waiting_for_input` until a client can answer; it is never denied because no client is attached. Until then, only `rho/thread/cancel` ends the wait.

Each job's record is a JSON file in `~/.rho/daemon/jobs/` (`$RHO_HOME/daemon/jobs/` with `RHO_HOME`), rewritten on every state change. Records are kept: nothing deletes them yet. A daemon lists the threads earlier daemons ran but cannot reopen them yet ([#1661](https://github.com/matthewyjiang/rho/issues/1661)), so it refuses to prompt them with `thread is not loaded in this daemon`.

Tools in a thread run with the daemon's own environment, minus provider credentials, as tools in every rho session do. A daemon that a frontend started automatically has that frontend's environment. Clients will send their own environment later ([#1659](https://github.com/matthewyjiang/rho/issues/1659)).

## Starting automatically

Frontends that use the daemon start it when none is running, so you rarely run `rho daemon` yourself. If several start at once, they agree on a single daemon. A frontend that starts one while the previous daemon is still shutting down can time out waiting for it; [#1657](https://github.com/matthewyjiang/rho/issues/1657) tracks waiting for the old daemon instead. The started daemon runs `rho daemon run` from the same `rho` executable in its own process group, so `Ctrl+C` in your terminal does not stop it.

## Versions

Every connection begins with a handshake that compares protocol versions. A frontend and daemon work together when their protocol major versions match. Otherwise the frontend reports which side to change:

- The daemon is older, for example after you update rho while it keeps running: run `rho daemon stop`, and the next connection starts the new version.
- The frontend is older: update rho.

## Socket and permissions

The daemon keeps its socket and info file in a runtime directory:

- `$RHO_HOME/run` when `RHO_HOME` is set. Each rho home gets its own daemon.
- Otherwise `$XDG_RUNTIME_DIR/rho` when `XDG_RUNTIME_DIR` is set to an absolute path, as on most Linux desktops.
- Otherwise `~/.rho/run`.

| File | Purpose |
| --- | --- |
| `daemon.sock` | The socket frontends connect to. |
| `daemon.json` | The running daemon's pid, version, protocol, start time, and socket. |
| `spawn.lock` | Lets concurrent frontends start exactly one daemon. |

Its lock, `daemon.lock`, lives in the rho home instead: `~/.rho/daemon/daemon.lock` (`$RHO_HOME/daemon/daemon.lock` with `RHO_HOME`). The running daemon holds it, so only one daemon runs per rho home even when two are started with different runtime directories.

Only your user can reach the daemon:

- The runtime directory and the lock's directory are created with mode `0700`. If it already exists, rho refuses to use it unless it is a real directory (not a symlink) that you own and that grants no group or other access.
- The socket has mode `0600`.
- The daemon checks the user id of every connecting process and closes connections from other users.
- The daemon listens on no TCP port.

Unix socket paths are limited to 107 bytes on Linux and 103 on macOS. If the socket path would be longer, rho fails with the limit and the actual length; point `RHO_HOME` (or `XDG_RUNTIME_DIR` for the default home) at a shorter directory.

The daemon is not supported on Windows yet ([#1665](https://github.com/matthewyjiang/rho/issues/1665)).

## Logs

When the daemon runs under systemd with stderr connected to the journal (`JOURNAL_STREAM` matches stderr), it logs to stderr and journald records it. Otherwise it appends to `~/.rho/logs/daemon.log` (`$RHO_HOME/logs/daemon.log` with `RHO_HOME`).

`RHO_LOG` sets the log filter with the same syntax as for other rho commands, for example `rho=debug`. The daemon defaults to `info`.

## Configuration changes

The daemon reads [configuration](/configuration) when it creates a session, so edits apply to new sessions without a restart. Daemon-level settings, which are the `RHO_LOG` filter, the socket location (`XDG_RUNTIME_DIR`, `RHO_HOME`), and the credential store, are read once at startup. Restart the daemon with `rho daemon stop` to change them.

## Running as a service

systemd user units and launchd agents for the daemon are planned ([#1663](https://github.com/matthewyjiang/rho/issues/1663), [#1664](https://github.com/matthewyjiang/rho/issues/1664)).
