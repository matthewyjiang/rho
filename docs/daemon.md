# Daemon

The rho daemon is one long-lived background process per user. It owns agent work so that frontends (the interactive TUI, `rho acp`, and other clients) can come and go without stopping it. Frontends talk to it over a private Unix socket using rho's versioned JSON-RPC protocol, published as the [`rho-protocol`](https://crates.io/crates/rho-protocol) crate.

## Commands

| Command | What it does |
| --- | --- |
| `rho daemon` | Run the daemon in the foreground until it is stopped. Same as `rho daemon run`. |
| `rho daemon status` | Show the running daemon's pid, version, protocol, uptime, jobs, and socket. Add `--json` for one JSON document. |
| `rho daemon stop` | Ask the daemon to shut down and wait for it to exit. |

`status` and `stop` never start a daemon. With none running, they print `rho daemon is not running` and exit with status 1.

Only one daemon runs per user. A second `rho daemon run` fails with the pid and version of the one already running.

The daemon shuts down gracefully on `rho daemon stop`, `SIGTERM`, or `SIGINT`: it stops accepting connections, finishes responses it is already sending, removes its socket, and exits with status 0.

## Starting automatically

Frontends that use the daemon start it when none is running, so you rarely run `rho daemon` yourself. If several start at once, they agree on a single daemon. The started daemon runs `rho daemon run` from the same `rho` executable in its own process group, so `Ctrl+C` in your terminal does not stop it.

## Versions

Every connection begins with a handshake that compares protocol versions. A frontend and daemon work together when their protocol major versions match. Otherwise the frontend reports which side to change:

- The daemon is older, for example after you update rho while it keeps running: run `rho daemon stop`, and the next connection starts the new version.
- The frontend is older: update rho.

## Socket and permissions

The daemon keeps its socket, lock, and info file in a runtime directory:

- `$RHO_HOME/run` when `RHO_HOME` is set. Each rho home gets its own daemon.
- Otherwise `$XDG_RUNTIME_DIR/rho` when `XDG_RUNTIME_DIR` is set to an absolute path, as on most Linux desktops.
- Otherwise `~/.rho/run`.

| File | Purpose |
| --- | --- |
| `daemon.sock` | The socket frontends connect to. |
| `daemon.lock` | Held by the running daemon; guarantees a single instance. |
| `daemon.json` | The running daemon's pid, version, protocol, start time, and socket. |
| `spawn.lock` | Lets concurrent frontends start exactly one daemon. |

Only your user can reach the daemon:

- The runtime directory is created with mode `0700`. If it already exists, rho refuses to use it unless it is a real directory (not a symlink) that you own and that grants no group or other access.
- The socket has mode `0600`.
- The daemon checks the user id of every connecting process and closes connections from other users.
- The daemon listens on no TCP port.

Unix socket paths are limited to 107 bytes on Linux and 103 on macOS. If the socket path would be longer, rho fails with the limit and the actual length; point `RHO_HOME` (or `XDG_RUNTIME_DIR` for the default home) at a shorter directory.

The daemon is not supported on Windows yet ([#1665](https://github.com/matthewyjiang/rho/issues/1665)).

## Logs

When the daemon runs under systemd (`JOURNAL_STREAM` is set), it logs to stderr and journald records it. Otherwise it appends to `~/.rho/logs/daemon.log` (`$RHO_HOME/logs/daemon.log` with `RHO_HOME`).

`RHO_LOG` sets the log filter with the same syntax as for other rho commands, for example `rho=debug`. The daemon defaults to `info`.

## Configuration changes

The daemon reads [configuration](/configuration) when it creates a session, so edits apply to new sessions without a restart. Daemon-level settings, which are the `RHO_LOG` filter and the socket location (`XDG_RUNTIME_DIR`, `RHO_HOME`), are read once at startup. Restart the daemon with `rho daemon stop` to change them.

## Running as a service

systemd user units and launchd agents for the daemon are planned ([#1663](https://github.com/matthewyjiang/rho/issues/1663), [#1664](https://github.com/matthewyjiang/rho/issues/1664)).
