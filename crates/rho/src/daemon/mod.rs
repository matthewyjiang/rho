//! The rho daemon: one long-lived process per user and rho home that frontends
//! reach over a private local endpoint using the `rho-protocol` JSON-RPC
//! protocol.
//!
//! This module owns the daemon's process plumbing: file layout and
//! permissions ([`DaemonPaths`]), the single-instance lock, the listener and
//! accept loop, client connection and auto-spawn, and restarting in place
//! (#1713): noticing that its executable was replaced ([`Executable`]) and
//! running the replacement in its place. It never learns what a session is:
//! what the daemon answers comes from the `rho_protocol::Service` its caller
//! passes in, which `app::daemon_host` implements. Protocol framing and the
//! handshake live in `rho-protocol`; `app::daemon_cli` wires the host to this
//! transport.
//!
//! Everything that differs between operating systems sits behind the
//! `platform` module: the endpoint, private files, detaching and stopping
//! the process, and how a restart hands over. Nothing else here, in the
//! host, or in a client branches on the platform.
//!
//! Configuration is read per session when a session is created, so config
//! edits apply to new sessions without a restart. Daemon-level settings (the
//! `RHO_LOG` filter, the socket location) are read once at startup and need a
//! restart.

mod client;
mod executable;
mod lock;
mod paths;
mod platform;
mod server;
mod shutdown;
mod spawn;
mod spawn_env;
#[cfg(test)]
pub(crate) mod test_support;

pub(crate) use client::{
    connect, connect_restarting, ConnectAs, ConnectError, Connection, DAEMON_RESTART_TIMEOUT,
};
pub(crate) use executable::{Executable, ProbeError};
pub(crate) use lock::{
    acquire as acquire_lock, holder as lock_holder, terminate_recorded_daemon, wait_until_released,
    DaemonLock, LockHolder,
};
pub(crate) use paths::{real_home, DaemonPaths, SocketFound, DAEMON_LOCATION_ENV};
// Named outside this module only by the Unix tests of what `rho doctor`
// reports.
#[cfg(all(test, unix))]
pub(crate) use platform::DaemonPathError;
pub(crate) use platform::{
    account_lock_path, create_private_dir, create_private_file, lock_account_file,
    stderr_is_journal_stream, sync_dir, FileIdentity, StopRequests,
};
pub(crate) use server::{bind, rebind, take_handoff, Served};
pub(crate) use shutdown::{Ending, Shutdown};
pub(crate) use spawn::{connect_or_spawn, DaemonSpawner, ProcessSpawner};
pub(crate) use spawn_env::{differing_variables, VariableKind};
