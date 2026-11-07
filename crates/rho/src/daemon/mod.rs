//! The rho daemon: one long-lived process per user and rho home that frontends
//! reach over a private Unix socket using the `rho-protocol` JSON-RPC
//! protocol.
//!
//! This module owns the daemon's process plumbing: file layout and
//! permissions ([`DaemonPaths`]), the single-instance lock, the listener and
//! accept loop, and client connection and auto-spawn. It never learns what a
//! session is: what the daemon answers comes from the `rho_protocol::Service`
//! its caller passes in, which `app::daemon_host` implements. Protocol framing
//! and the handshake live in `rho-protocol`; `app::daemon_cli` wires the host
//! to this transport.
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
#[cfg(test)]
mod test_support;

pub(crate) use client::{cli_client_info, connect, ConnectError, Connection};
pub(crate) use lock::{
    acquire as acquire_lock, terminate_recorded_daemon, wait_until_released, DaemonLock,
};
pub(crate) use paths::{real_home, DaemonPaths};
pub(crate) use server::bind;
