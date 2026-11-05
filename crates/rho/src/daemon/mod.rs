//! The rho daemon: one long-lived process per user and rho home that frontends
//! reach over a private Unix socket using the `rho-protocol` JSON-RPC
//! protocol.
//!
//! This module owns the daemon's process plumbing: file layout and
//! permissions ([`DaemonPaths`]), the single-instance lock, the listener, and
//! client connection and auto-spawn. Protocol framing and the handshake live in
//! `rho-protocol`; the `rho daemon` CLI lives in `app::daemon_cli`.
//!
//! Configuration is read per session when a session is created, so config
//! edits apply to new sessions without a restart. Daemon-level settings (the
//! `RHO_LOG` filter, the socket location) are read once at startup and need a
//! restart.

mod client;
mod lock;
mod paths;
mod server;
#[expect(
    dead_code,
    reason = "frontends (the TUI, `rho acp`) start the daemon through this in follow-up PRs; \
              until then production code never spawns and tests never use ProcessSpawner"
)]
mod spawn;

pub(crate) use client::{cli_client_info, connect, ConnectError};
pub(crate) use lock::{acquire as acquire_lock, terminate_recorded_daemon, wait_until_released};
pub(crate) use paths::DaemonPaths;
pub(crate) use server::bind;
