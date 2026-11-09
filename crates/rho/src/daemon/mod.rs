//! The rho daemon: one long-lived process per user and rho home that frontends
//! reach over a private Unix socket using the `rho-protocol` JSON-RPC
//! protocol.
//!
//! This module owns the daemon's process plumbing: file layout and
//! permissions ([`DaemonPaths`]), the single-instance lock, the listener and
//! accept loop, client connection and auto-spawn, and re-executing in place
//! (#1713): noticing that its executable was replaced ([`Executable`]) and
//! handing the lock and the listening socket to the binary it re-executes. It never learns what a
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
mod executable;
mod handoff;
mod lock;
mod paths;
mod server;
mod shutdown;
mod spawn;
#[cfg(test)]
mod test_support;

pub(crate) use client::{
    cli_client_info, connect, ConnectError, Connection, DAEMON_RESTART_TIMEOUT,
};
pub(crate) use executable::{Executable, FileIdentity, ProbeError};
pub(crate) use handoff::{take as take_handoff, Handoff};
pub(crate) use lock::{
    acquire as acquire_lock, terminate_recorded_daemon, wait_until_released, DaemonLock,
};
pub(crate) use paths::{real_home, DaemonPaths};
pub(crate) use server::{bind, rebind, Served};
pub(crate) use shutdown::{Ending, Shutdown};
pub(crate) use spawn::{connect_or_spawn, DaemonSpawner, ProcessSpawner};
