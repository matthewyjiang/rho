# Daemon

The rho daemon is one long-lived background process per user and rho home (`~/.rho`, or `$RHO_HOME`). It owns agent work so that frontends (the interactive TUI, `rho acp`, and other clients) can come and go without stopping it. Frontends talk to it over a private Unix socket using rho's versioned JSON-RPC protocol, published as the [`rho-protocol`](https://crates.io/crates/rho-protocol) crate.

## Commands

| Command | What it does |
| --- | --- |
| `rho daemon` | Run the daemon in the foreground until it is stopped. Same as `rho daemon run`. |
| `rho daemon status` | Show the running daemon's pid, version, protocol, uptime, jobs, and socket. Add `--json` for one JSON document. |
| `rho daemon stop` | Ask the daemon to shut down and wait for it to exit. |

`status` and `stop` never start a daemon. With none running, they print `rho daemon is not running` and exit with status 1.

Only one daemon runs per rho home. A second `rho daemon run` fails with the pid, version, and socket of the one already running.

The daemon shuts down gracefully on `rho daemon stop`, `SIGTERM`, or `SIGINT`: it stops accepting connections and reading requests, answers the requests it has already received, and removes its socket. It then cancels the [turns](#sessions-and-jobs) still running, waits up to 4 seconds for them to save, records them `interrupted`, and exits with status 0. A request that never finishes holds that shutdown open. A `SIGTERM` or `SIGINT` that arrives once shutdown is under way, whether a second signal or any signal after `rho daemon stop`, makes the daemon exit at once with status 1; the next daemon to start removes the socket it left behind and records the turns that daemon left open as `interrupted`.

## Sessions and jobs

A session is a conversation: its saved transcript and, while the daemon has it loaded, its live runtime. Requests name a session by its session id, the id `rho sessions` and `/resume` show. A loaded session's turns run in the daemon, so a turn keeps running when the client that started it disconnects, and another client can follow it later. The daemon tracks every unit of work it runs as a job with its own id; today every job is the turns of one session.

The [`rho-protocol` `methods` module](https://docs.rs/rho-protocol/latest/rho_protocol/methods/) is the full method reference. The session and job methods:

| Method | Params | What it does |
| --- | --- | --- |
| `rho/session/new` | `cwd` | Opens a new session whose workspace is `cwd`, an absolute path to an existing directory. Returns its session id. |
| `rho/session/prompt` | `session`, `text` | Starts a turn and returns once the turn has saved the prompt. A session runs one turn at a time; a prompt while a turn is under way is refused with `SessionBusy`. |
| `rho/session/cancel` | `session` | Cancels the turn under way, if any. The turn ends `cancelled`, or `interrupted` when the daemon is stopping. |
| `rho/session/steer` | `session`, `text` | Adds `text` to the running turn's next model request. Refused with `TurnNotRunning` when no model request is running. |
| `rho/jobs/list` | | Lists every job the daemon knows, including those earlier daemons ran, each with its kind, state, workspace, and session. |

A prompt whose turn ends before it saves the prompt returns an error instead of a result:

- `TurnCancelled` (-32010) when a cancel arrived before the turn began. Nothing is saved and the model is not called.
- `DaemonStopping` (-32007) when the daemon starts stopping while the prompt waits for its turn to save it.
- `TurnFailed` (-32009) with the reason when the turn failed, such as unreadable provider credentials. A turn that stopped unexpectedly answers `the turn stopped unexpectedly; see the daemon log`.

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

A new prompt moves a session's job from any state but `running` and `waiting_for_input` back to `running`. `rho daemon status` counts the jobs that are `running` or `waiting_for_input`.

Clients cannot yet watch a turn's events ([#1657](https://github.com/matthewyjiang/rho/issues/1657)) or answer its requests ([#1660](https://github.com/matthewyjiang/rho/issues/1660)). A turn that asks for approval or input waits in `waiting_for_input` until a client can answer; it is never denied because no client is attached. Until then, only `rho/session/cancel` ends the wait.

Each job's record is a JSON file in `~/.rho/daemon/jobs/` (`$RHO_HOME/daemon/jobs/` with `RHO_HOME`), rewritten on every state change. Records are kept: nothing deletes them yet, so a daemon lists the jobs earlier daemons ran. A daemon serves only the sessions it opened itself; it cannot load a saved session yet ([#1661](https://github.com/matthewyjiang/rho/issues/1661)). A request for a saved session it has not loaded, such as one an earlier daemon ran or one the TUI saved, is refused with `SessionNotLoaded` (-32003); a session id no saved or loaded session has is refused with `SessionNotFound` (-32002).

Tools in a session run with the daemon's own environment, minus provider credentials, as tools in every rho session do. A daemon that a frontend started automatically has that frontend's environment. Clients will send their own environment later ([#1659](https://github.com/matthewyjiang/rho/issues/1659)).

## Starting automatically

Frontends that use the daemon start it when none is running, so you rarely run `rho daemon` yourself. If several start at once, they agree on a single daemon. A frontend that starts one while the previous daemon is still shutting down can time out waiting for it; [#1657](https://github.com/matthewyjiang/rho/issues/1657) tracks waiting for the old daemon instead. The started daemon runs `rho daemon run` from the same `rho` executable in its own process group, so `Ctrl+C` in your terminal does not stop it.

## Versions

Every connection begins with a handshake that compares protocol versions. A frontend and daemon work together when their protocol major versions match. Otherwise the frontend reports which side to change:

- The daemon is older, for example after you update rho while it keeps running: run `rho daemon stop`, and the next connection starts the new version.
- The frontend is older: update rho.

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
